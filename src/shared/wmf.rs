//! Metafile (WMF/EMF) rasterization to PNG.
//!
//! Two implementations with the same signature
//! `metafile_to_png(&[u8]) -> Option<Vec<u8>>`:
//!
//! - **Windows** ([`windows_backend`]): GDI `SetWinMetaFileBits` upgrades WMF
//!   to EMF, then `gdiplus.dll` (loaded dynamically) renders the EMF onto an
//!   ARGB bitmap; we crop to the content bounding box and re-encode as PNG.
//!   Verified against real Word output.
//! - **Other platforms** ([`unix_backend`]): shell out to LibreOffice
//!   (`soffice --headless --convert-to png`). `soffice` is **not** a hard
//!   dependency: when no binary is found on `PATH` the conversion returns
//!   `None` and the caller keeps the original bytes (with a `log::warn!`).
//!   The probe is cached per process so a missing binary doesn't pay the
//!   ~1s cold-start on every metafile in a document.
//!
//! Both backends return `Option<Vec<u8>>`; callers retain the original bytes
//! on `None`.

#[cfg(windows)]
mod windows_backend {
    //! Windows backend: GDI upgrade + GDI+ rasterize + custom PNG encode.

    use std::ffi::{OsStr, c_void};
    use std::io::Write as _;
    use std::os::windows::ffi::OsStrExt as _;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Hdc = *mut c_void;
    type HMetaFile = *mut c_void;
    type Handle = *mut c_void;

    const MM_ANISOTROPIC: i32 = 8;
    /// GDI+ pixel format 32bpp ARGB (memory byte order B,G,R,A).
    const PIXEL_FORMAT_32BPP_ARGB: u32 = 0x0026_200A;
    const IMAGE_LOCK_MODE_READ: u32 = 1;
    const GDIP_STATUS_OK: i32 = 0;
    /// Opaque white, GDI+ ARGB form.
    const ARGB_WHITE: u32 = 0xFFFF_FFFF;

    /// Longest edge of the rendered bitmap before cropping, in pixels.
    const TARGET_LONG_EDGE: f64 = 2000.0;
    /// Hard cap on either rendered dimension, bounding memory use against
    /// malformed headers that declare huge bounds.
    const MAX_DIM: i32 = 4096;
    /// A pixel darker than this in any channel counts as content.
    const CONTENT_THRESHOLD: u8 = 240;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct MetafilePict {
        mm: i32,
        x_ext: i32,
        y_ext: i32,
        h_mf: *mut c_void,
    }

    #[repr(C)]
    struct GdiplusStartupInput {
        version: u32,
        debug_event_callback: *mut c_void,
        suppress_background_thread: i32,
        suppress_external_codecs: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct BitmapData {
        width: u32,
        height: u32,
        stride: i32,
        pixel_format: u32,
        scan0: *mut u8,
        reserved: *mut c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RectI {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(hwnd: *mut c_void) -> Hdc;
        fn ReleaseDC(hwnd: *mut c_void, hdc: Hdc) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn SetWinMetaFileBits(
            cbbuffer: u32,
            lpbuffer: *const u8,
            hdcref: Hdc,
            lpmfp: *const MetafilePict,
        ) -> HMetaFile;
        fn GetEnhMetaFileBits(hemf: HMetaFile, cbbuffer: u32, lpdata: *mut u8) -> u32;
        fn DeleteEnhMetaFile(hemf: HMetaFile) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(lpfilename: *const u16) -> Handle;
        fn GetProcAddress(hmodule: Handle, lpprocname: *const u8) -> *mut c_void;
        fn FreeLibrary(hmodule: Handle) -> i32;
    }

    type GdiplusStartupFn =
        unsafe extern "system" fn(*mut usize, *const GdiplusStartupInput, *mut c_void) -> i32;
    type GdiplusShutdownFn = unsafe extern "system" fn(usize);
    type CreateMetafileFromFileFn = unsafe extern "system" fn(*const u16, *mut *mut c_void) -> i32;
    type GetImageSizeFn = unsafe extern "system" fn(*mut c_void, *mut u32) -> i32;
    type CreateBitmapFromScan0Fn =
        unsafe extern "system" fn(i32, i32, i32, u32, *const u8, *mut *mut c_void) -> i32;
    type GetImageGraphicsContextFn =
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32;
    type GraphicsClearFn = unsafe extern "system" fn(*mut c_void, u32) -> i32;
    type DrawImageRectIFn =
        unsafe extern "system" fn(*mut c_void, *mut c_void, i32, i32, i32, i32) -> i32;
    type BitmapLockBitsFn =
        unsafe extern "system" fn(*mut c_void, *const RectI, u32, u32, *mut BitmapData) -> i32;
    type BitmapUnlockBitsFn = unsafe extern "system" fn(*mut c_void, *mut BitmapData) -> i32;
    type DestroyObjFn = unsafe extern "system" fn(*mut c_void) -> i32;
    type GetImageTypeFn = unsafe extern "system" fn(*mut c_void, *mut i32) -> i32;
    type GetImageBoundsFn = unsafe extern "system" fn(*mut c_void, *mut [f32; 4], *mut i32) -> i32;

    /// GDI+ entry points resolved once from `gdiplus.dll`. The library is
    /// loaded dynamically: it ships with Windows but has no guaranteed presence
    /// (Server Core, Nano), and a missing DLL must not break conversion.
    /// Function pointers are plain addresses; GDI+ serializes its own global
    /// state, so sharing them across threads is safe here.
    struct Gdiplus {
        module: Handle,
        startup: GdiplusStartupFn,
        shutdown: GdiplusShutdownFn,
        create_metafile_from_file: CreateMetafileFromFileFn,
        get_image_width: GetImageSizeFn,
        get_image_height: GetImageSizeFn,
        create_bitmap_from_scan0: CreateBitmapFromScan0Fn,
        get_image_graphics_context: GetImageGraphicsContextFn,
        graphics_clear: GraphicsClearFn,
        draw_image_rect_i: DrawImageRectIFn,
        bitmap_lock_bits: BitmapLockBitsFn,
        bitmap_unlock_bits: BitmapUnlockBitsFn,
        dispose_image: DestroyObjFn,
        delete_graphics: DestroyObjFn,
        get_image_type: GetImageTypeFn,
        get_image_bounds: GetImageBoundsFn,
    }

    // Safety: the struct holds module/fn addresses obtained from the OS loader.
    // Calls through them take raw pointers created on the calling thread, and
    // GDI+ serializes its own global state, so concurrent use is sound.
    unsafe impl Send for Gdiplus {}
    unsafe impl Sync for Gdiplus {}

    fn gdiplus() -> Option<&'static Gdiplus> {
        static ONCE: std::sync::OnceLock<Option<Gdiplus>> = std::sync::OnceLock::new();
        ONCE.get_or_init(load_gdiplus).as_ref()
    }

    fn load_gdiplus() -> Option<Gdiplus> {
        let name: Vec<u16> =
            OsStr::new("gdiplus.dll").encode_wide().chain(std::iter::once(0)).collect();
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module.is_null() {
            return None;
        }
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let p = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
                if p.is_null() {
                    unsafe { FreeLibrary(module) };
                    return None;
                }
                unsafe { std::mem::transmute::<*mut c_void, $ty>(p) }
            }};
        }
        Some(Gdiplus {
            module,
            startup: sym!("GdiplusStartup", GdiplusStartupFn),
            shutdown: sym!("GdiplusShutdown", GdiplusShutdownFn),
            create_metafile_from_file: sym!("GdipCreateMetafileFromFile", CreateMetafileFromFileFn),
            get_image_width: sym!("GdipGetImageWidth", GetImageSizeFn),
            get_image_height: sym!("GdipGetImageHeight", GetImageSizeFn),
            create_bitmap_from_scan0: sym!("GdipCreateBitmapFromScan0", CreateBitmapFromScan0Fn),
            get_image_graphics_context: sym!(
                "GdipGetImageGraphicsContext",
                GetImageGraphicsContextFn
            ),
            graphics_clear: sym!("GdipGraphicsClear", GraphicsClearFn),
            draw_image_rect_i: sym!("GdipDrawImageRectI", DrawImageRectIFn),
            bitmap_lock_bits: sym!("GdipBitmapLockBits", BitmapLockBitsFn),
            bitmap_unlock_bits: sym!("GdipBitmapUnlockBits", BitmapUnlockBitsFn),
            dispose_image: sym!("GdipDisposeImage", DestroyObjFn),
            delete_graphics: sym!("GdipDeleteGraphics", DestroyObjFn),
            get_image_type: sym!("GdipGetImageType", GetImageTypeFn),
            get_image_bounds: sym!("GdipGetImageBounds", GetImageBoundsFn),
        })
    }

    /// Owned GDI+ object handle: disposes on scope exit.
    struct GpObj {
        p: *mut c_void,
        destroy: DestroyObjFn,
    }

    impl Drop for GpObj {
        fn drop(&mut self) {
            unsafe {
                (self.destroy)(self.p);
            }
        }
    }

    /// Rasterize a WMF or EMF payload to PNG. Returns `None` when the payload
    /// cannot be played, renders empty, or exceeds the dimension cap; callers
    /// keep the original bytes in that case.
    pub(crate) fn metafile_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
        let emf = metafile_bytes(bytes)?;
        let gp = gdiplus()?;
        let mut token = 0usize;
        let input = GdiplusStartupInput {
            version: 1,
            debug_event_callback: null_mut(),
            suppress_background_thread: 0,
            suppress_external_codecs: 0,
        };
        if unsafe { (gp.startup)(&mut token, &input, null_mut()) } != GDIP_STATUS_OK {
            return None;
        }
        let rendered = render(gp, &emf);
        unsafe {
            (gp.shutdown)(token);
        }
        rendered
    }

    /// The payload as an enhanced metafile: EMF input passes through; WMF is
    /// upgraded by GDI. A WMF may carry a 22-byte placeable header (key
    /// `0x9AC6CDD7`) that `SetWinMetaFileBits` does not want; it is stripped.
    fn metafile_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
        if bytes.len() >= 88
            && u32::from_le_bytes(bytes.get(40..44)?.try_into().ok()?) == 0x464D_4520
        {
            return Some(bytes.to_vec());
        }
        let mut data = bytes;
        if bytes.len() >= 22 && u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) == 0x9AC6_CDD7
        {
            data = &bytes[22..];
        }
        if data.len() < 18 {
            return None;
        }
        // 20cm x 20cm reference extent in hundredths of a millimeter; the final
        // size comes from the canvas rect. A real screen DC is required as the
        // reference: with NULL the converted EMF header carries zero
        // szlDevice/szlMillimeters and playback scales to nothing.
        let pict = MetafilePict { mm: MM_ANISOTROPIC, x_ext: 5080, y_ext: 5080, h_mf: null_mut() };
        let screen = unsafe { GetDC(null_mut()) };
        if screen.is_null() {
            return None;
        }
        let hemf = unsafe { SetWinMetaFileBits(data.len() as u32, data.as_ptr(), screen, &pict) };
        unsafe { ReleaseDC(null_mut(), screen) };
        if hemf.is_null() {
            return None;
        }
        let len = unsafe { GetEnhMetaFileBits(hemf, 0, null_mut()) };
        if len == 0 {
            unsafe { DeleteEnhMetaFile(hemf) };
            return None;
        }
        let mut out = vec![0u8; len as usize];
        let got = unsafe { GetEnhMetaFileBits(hemf, len, out.as_mut_ptr()) };
        unsafe { DeleteEnhMetaFile(hemf) };
        (got == len).then_some(out)
    }

    fn render(gp: &Gdiplus, emf: &[u8]) -> Option<Vec<u8>> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "anydoc-meta-{}-{}.emf",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, emf).ok()?;
        let result = render_from_file(gp, path.as_os_str());
        let _ = std::fs::remove_file(&path);
        result
    }

    fn render_from_file(gp: &Gdiplus, path: &OsStr) -> Option<Vec<u8>> {
        let wide: Vec<u16> = path.encode_wide().chain(std::iter::once(0)).collect();
        let mut image: *mut c_void = null_mut();
        let status = unsafe { (gp.create_metafile_from_file)(wide.as_ptr(), &mut image) };
        if status != GDIP_STATUS_OK || image.is_null() {
            eprintln!("dbg: create_metafile_from_file status={status}");
            return None;
        }
        let image = GpObj { p: image, destroy: gp.dispose_image };
        {
            let (mut ty, mut bounds, mut unit) = (0i32, [0f32; 4], 0i32);
            let s1 = unsafe { (gp.get_image_type)(image.p, &mut ty) };
            let s2 = unsafe { (gp.get_image_bounds)(image.p, &mut bounds, &mut unit) };
            eprintln!("dbg: image type={ty} st={s1} bounds={bounds:?} unit={unit} st={s2}");
        }

        // The frame's pixel size sets the canvas aspect; the metafile's own
        // window/viewport mapping letterboxes the content inside it.
        let (mut w, mut h) = (0u32, 0u32);
        let status = unsafe { (gp.get_image_width)(image.p, &mut w) };
        let status2 = unsafe { (gp.get_image_height)(image.p, &mut h) };
        if status != GDIP_STATUS_OK || status2 != GDIP_STATUS_OK || w == 0 || h == 0 {
            eprintln!("dbg: image size status={status}/{status2} w={w} h={h}");
            return None;
        }
        let scale = TARGET_LONG_EDGE / w.max(h) as f64;
        let px_w = ((w as f64) * scale).round().clamp(1.0, MAX_DIM as f64) as i32;
        let px_h = ((h as f64) * scale).round().clamp(1.0, MAX_DIM as f64) as i32;

        let mut bitmap: *mut c_void = null_mut();
        let status = unsafe {
            (gp.create_bitmap_from_scan0)(
                px_w,
                px_h,
                0,
                PIXEL_FORMAT_32BPP_ARGB,
                null_mut(),
                &mut bitmap,
            )
        };
        if status != GDIP_STATUS_OK || bitmap.is_null() {
            return None;
        }
        let bitmap = GpObj { p: bitmap, destroy: gp.dispose_image };

        let mut graphics: *mut c_void = null_mut();
        let status = unsafe { (gp.get_image_graphics_context)(bitmap.p, &mut graphics) };
        if status != GDIP_STATUS_OK || graphics.is_null() {
            return None;
        }
        let graphics = GpObj { p: graphics, destroy: gp.delete_graphics };

        // White base so transparent regions survive the content scan.
        if unsafe { (gp.graphics_clear)(graphics.p, ARGB_WHITE) } != GDIP_STATUS_OK {
            return None;
        }
        let dst = unsafe { (gp.draw_image_rect_i)(graphics.p, image.p, 0, 0, px_w, px_h) };
        if dst != GDIP_STATUS_OK {
            eprintln!("dbg: draw_image status={dst}");
            return None;
        }

        let mut data = BitmapData::default();
        let rc = RectI { x: 0, y: 0, w: px_w, h: px_h };
        let status = unsafe {
            (gp.bitmap_lock_bits)(
                bitmap.p,
                &rc,
                IMAGE_LOCK_MODE_READ,
                PIXEL_FORMAT_32BPP_ARGB,
                &mut data,
            )
        };
        if status != GDIP_STATUS_OK {
            return None;
        }
        let rgba = unsafe { lockbits_to_rgba(&data) };
        unsafe { (gp.bitmap_unlock_bits)(bitmap.p, &mut data) };
        let Some(rgba) = rgba else {
            eprintln!("dbg: lockbits_to_rgba none");
            return None;
        };
        let Some((x0, y0, cw, ch)) = content_bounds(&rgba, px_w as usize, px_h as usize) else {
            eprintln!("dbg: content empty ({}x{px_h})", px_w);
            return None;
        };
        Some(encode_png(cw as u32, ch as u32, &crop_rgba(&rgba, px_w as usize, x0, y0, cw, ch)))
    }

    /// Copy the locked bitmap into a tight top-down RGBA buffer. `stride` may
    /// be negative (bottom-up layout); `scan0` always addresses the visual
    /// top row by `scan0 + y * stride`.
    unsafe fn lockbits_to_rgba(data: &BitmapData) -> Option<Vec<u8>> {
        if data.scan0.is_null() || data.stride == 0 || data.width == 0 || data.height == 0 {
            return None;
        }
        let (w, h) = (data.width as usize, data.height as usize);
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h as isize {
            let row = unsafe { data.scan0.offset(y * data.stride as isize) };
            for x in 0..w {
                let s = unsafe { row.add(x * 4) };
                let d = (y as usize * w + x) * 4;
                out[d] = unsafe { *s.add(2) };
                out[d + 1] = unsafe { *s.add(1) };
                out[d + 2] = unsafe { *s };
                out[d + 3] = 0xFF;
            }
        }
        Some(out)
    }

    /// Bounding box of every non-white pixel, padded by 4% on each side like
    /// the external conversion script this replaces. `None` when fully blank.
    fn content_bounds(pixels: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        let mut min_x = w;
        let mut min_y = h;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        for y in 0..h {
            let row = y * w * 4;
            for x in 0..w {
                let p = row + x * 4;
                if pixels[p] < CONTENT_THRESHOLD
                    || pixels[p + 1] < CONTENT_THRESHOLD
                    || pixels[p + 2] < CONTENT_THRESHOLD
                {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        if max_x < min_x {
            return None;
        }
        let mx = (w / 25).max(2);
        let my = (h / 25).max(2);
        let x0 = min_x.saturating_sub(mx);
        let y0 = min_y.saturating_sub(my);
        let x1 = (max_x + mx + 1).min(w);
        let y1 = (max_y + my + 1).min(h);
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    /// Copy the crop out of a tight top-down RGBA buffer.
    fn crop_rgba(rgba: &[u8], w: usize, x0: usize, y0: usize, cw: usize, ch: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(cw * ch * 4);
        for y in y0..y0 + ch {
            let start = (y * w + x0) * 4;
            out.extend_from_slice(&rgba[start..start + cw * 4]);
        }
        out
    }

    fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(rgba.len() / 2 + 64);
        out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_chunk(&mut out, b"IHDR", &ihdr);

        let stride = width as usize * 4;
        let mut raw = Vec::with_capacity((stride + 1) * height as usize);
        for row in 0..height as usize {
            raw.push(0);
            raw.extend_from_slice(&rgba[row * stride..(row + 1) * stride]);
        }
        let mut idat = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        idat.write_all(&raw).expect("zlib encoder writes to memory");
        push_chunk(&mut out, b"IDAT", &idat.finish().expect("zlib finish on memory buffer"));
        push_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn push_chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        let crc = crc32_update(crc32_update(0xFFFF_FFFF, name), data) ^ 0xFFFF_FFFF;
        out.extend_from_slice(&crc.to_be_bytes());
    }

    const CRC_TABLE: [u32; 256] = build_crc_table();

    const fn build_crc_table() -> [u32; 256] {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    }

    fn crc32_update(crc: u32, data: &[u8]) -> u32 {
        let mut c = crc;
        for &b in data {
            c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        c
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Read as _;

        /// Parse the PNG this module wrote back into RGBA pixels.
        fn decode_png(png: &[u8]) -> (u32, u32, Vec<u8>) {
            assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
            let (mut w, mut h, mut idat) = (0u32, 0u32, Vec::new());
            let mut pos = 8;
            while pos + 8 <= png.len() {
                let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
                let name = &png[pos + 4..pos + 8];
                let data = &png[pos + 8..pos + 8 + len];
                match name {
                    b"IHDR" => {
                        w = u32::from_be_bytes(data[..4].try_into().unwrap());
                        h = u32::from_be_bytes(data[4..8].try_into().unwrap());
                    }
                    b"IDAT" => idat.extend_from_slice(data),
                    b"IEND" => break,
                    _ => {}
                }
                pos += 12 + len;
            }
            let mut raw = Vec::new();
            flate2::read::ZlibDecoder::new(&idat[..]).read_to_end(&mut raw).unwrap();
            let stride = w as usize * 4;
            assert_eq!(raw.len(), (stride + 1) * h as usize);
            let mut rgba = Vec::with_capacity(stride * h as usize);
            for y in 0..h as usize {
                assert_eq!(raw[y * (stride + 1)], 0, "only filter type 0 is emitted");
                rgba.extend_from_slice(&raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)]);
            }
            (w, h, rgba)
        }

        fn has_pixel(rgba: &[u8], pred: impl Fn(u8, u8, u8) -> bool) -> bool {
            rgba.chunks_exact(4).any(|p| pred(p[0], p[1], p[2]))
        }

        #[test]
        fn crc32_matches_known_vector() {
            assert_eq!(crc32_update(0xFFFF_FFFF, b"123456789") ^ 0xFFFF_FFFF, 0xCBF4_3926);
        }

        /// Real WMF emitted by Word 2010 (placeable header + standard body).
        /// Stored under `tests/fixtures/wmf/asset.wmf` so the metafile->PNG
        /// upgrade path (`SetWinMetaFileBits` -> GDI+ bitmap) is exercised on
        /// the same kind of bytes the loader sees in production.
        const WMF_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/wmf/asset.wmf");
        /// Real EMF recorded via `System.Drawing.Imaging.Metafile`; same
        /// drawings as the WMF, different on-disk format.
        const EMF_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/wmf/asset.emf");

        #[test]
        fn wmf_renders_to_png_with_content() {
            let png = metafile_to_png(WMF_FIXTURE).expect("real WMF must play");
            let (w, h, rgba) = decode_png(&png);
            assert!(w > 0 && h > 0 && w <= 4096 && h <= 4096);
            // Real WMFs in the corpus are line drawings: look for any non-white
            // pixel (they all use thin black strokes, not solid fills).
            assert!(
                has_pixel(&rgba, |_, _, _| true)
                    && rgba.chunks_exact(4).any(|p| {
                        p[0] < CONTENT_THRESHOLD
                            || p[1] < CONTENT_THRESHOLD
                            || p[2] < CONTENT_THRESHOLD
                    }),
                "rendered PNG is blank"
            );
        }

        #[test]
        fn junk_metafile_returns_none() {
            assert!(metafile_to_png(&[0u8; 64]).is_none());
            assert!(metafile_to_png(&[]).is_none());
        }

        #[test]
        fn emf_renders_to_png_with_content() {
            let png = metafile_to_png(EMF_FIXTURE).expect("real EMF must play");
            let (_, _, rgba) = decode_png(&png);
            // Same expectation as the WMF test: real EMFs are line drawings,
            // not solid fills, so we look for any non-white pixel.
            assert!(
                rgba.chunks_exact(4).any(|p| {
                    p[0] < CONTENT_THRESHOLD || p[1] < CONTENT_THRESHOLD || p[2] < CONTENT_THRESHOLD
                }),
                "rendered PNG is blank"
            );
        }
    }
}

#[cfg(not(windows))]
mod unix_backend {
    //! Non-Windows backend: shell out to `soffice` / `libreoffice`.
    //!
    //! `soffice` is **not** a hard dependency. The first invocation probes the
    //! `PATH` for either `soffice` or `libreoffice`; the result is cached so a
    //! missing binary does not pay the cold-start cost on every metafile in a
    //! document. When absent the conversion returns `None` and the caller
    //! retains the original bytes with a `log::warn!`.

    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Probe `PATH` once for `soffice` or `libreoffice`; cache the result.
    fn soffice_command() -> Option<&'static str> {
        static ONCE: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            for &candidate in &["soffice", "libreoffice"] {
                if probe(candidate) {
                    return Some(candidate);
                }
            }
            None
        })
        .as_ref()
        .copied()
    }

    fn probe(cmd: &str) -> bool {
        // `--headless --version` exits quickly without touching a display;
        // `--norestore` and `--nofirststartwizard` keep it non-interactive on
        // first-run installs that would otherwise pop a dialog.
        Command::new(cmd)
            .arg("--headless")
            .arg("--norestore")
            .arg("--nofirststartwizard")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// EMF has the `" EMF"` signature at byte 40. Anything else we hand to
    /// LibreOffice as WMF; the extension only matters for the output filename
    /// (`--convert-to png` writes `<stem>.png`).
    fn extension(bytes: &[u8]) -> &'static str {
        if bytes.len() >= 44 {
            let sig = u32::from_le_bytes(bytes[40..44].try_into().unwrap_or([0; 4]));
            if sig == 0x464D_4520 {
                return "emf";
            }
        }
        "wmf"
    }

    pub(crate) fn metafile_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
        let cmd = soffice_command()?;
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let work = std::env::temp_dir().join(format!("anydoc-meta-{pid}-{counter}"));
        // Best-effort pre-clean: a previous crashed run may have left this dir
        // behind. `create_dir_all` would otherwise be a no-op on the leftover.
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).ok()?;

        let ext = extension(bytes);
        let input = work.join(format!("input.{ext}"));
        std::fs::write(&input, bytes).ok()?;

        let status = Command::new(cmd)
            .arg("--headless")
            .arg("--norestore")
            .arg("--nofirststartwizard")
            .arg("--convert-to")
            .arg("png")
            .arg("--outdir")
            .arg(&work)
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
        if !matches!(status, Some(s) if s.success()) {
            cleanup(&work);
            return None;
        }
        let output = work.join("input.png");
        let png = std::fs::read(&output).ok();
        cleanup(&work);
        png
    }

    fn cleanup(work: &std::path::Path) {
        let _ = std::fs::remove_dir_all(work);
    }
}

#[cfg(not(windows))]
pub(crate) use unix_backend::metafile_to_png;
#[cfg(windows)]
pub(crate) use windows_backend::metafile_to_png;
