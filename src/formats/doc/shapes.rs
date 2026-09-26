//! Floating shapes of a `.doc`: the `Plcspa` anchor table plus the OfficeArt
//! drawing in `DggInfo`.
//!
//! A picture Word wraps with text is not an inline picture: the text stream
//! carries no picture character for it. Instead the shape is anchored to a CP
//! by `Plcspa` (one `FSPA` per shape, holding the shape's `spid`), while the
//! drawing lives in the `DggInfo` blob of the Table stream: a
//! `BStoreContainer` of `FBSE` entries that point at the picture bytes, and one
//! `SpContainer` per shape whose `FSP` carries the `spid` and whose `FOPT`
//! carries `pib`, the 1-based index of the picture it displays in the BStore.

use crate::package::limits;
use crate::shared::binary::get_u32;
use crate::shared::officeart::{Blip, decode_blip, record_at};

/// FIB offsets into `fibRgFcLcb97` (Word 97 layout): `fcPlcspaMom` (the main
/// document's shape anchors) and `fcDggInfo` (the OfficeArt drawing).
const FIB_FC_SPA_MOM: usize = 0x1DA;
const FIB_FC_DGG_INFO: usize = 0x22A;

/// `FSPA`: `spid` plus the anchor rectangle and wrapping flags.
const FSPA_LEN: usize = 26;

const RT_FBSE: u16 = 0xF007;
const RT_FSP: u16 = 0xF00A;
const RT_FOPT: u16 = 0xF00B;

/// `pib`: the BStore index of the picture a shape displays.
const PROP_PIB: u16 = 0x0104;

/// A picture anchored in the text stream by a floating shape.
pub(super) struct AnchoredPicture {
    /// The CP whose paragraph the shape is anchored to.
    pub(super) cp: u32,
    pub(super) media_type: String,
    pub(super) part: String,
    pub(super) bytes: Vec<u8>,
}

/// Collect the pictures of the floating shapes in the main document.
pub(super) fn parse(word_doc: &[u8], table: &[u8], data: &[u8]) -> Vec<AnchoredPicture> {
    let Some(anchors) = plc_spa(word_doc, table) else {
        return Vec::new();
    };
    let dgg = get_u32(word_doc, FIB_FC_DGG_INFO).zip(get_u32(word_doc, FIB_FC_DGG_INFO + 4));
    let drawing = match dgg {
        Some((fc, lcb)) => Drawing::parse(table, fc as usize, lcb as usize),
        None => Drawing::default(),
    };
    let mut out = Vec::new();
    for (cp, spid) in anchors {
        // spid -> pib -> BStore entry -> picture bytes.
        let Some(pib) = drawing.pib_of(spid) else {
            continue;
        };
        let Some(offset) = drawing.blip_offset(pib) else {
            continue;
        };
        let Some(blip) = blip_at(word_doc, data, offset, limits::MAX_ENTRY_BYTES as usize) else {
            continue;
        };
        let part = format!("data/shape{}.{}", out.len(), blip.extension);
        out.push(AnchoredPicture {
            cp,
            media_type: blip.media_type.to_string(),
            part,
            bytes: blip.bytes.into_owned(),
        });
    }
    out
}

/// The main document's shape anchors: (CP, spid) per `FSPA`.
fn plc_spa(word_doc: &[u8], table: &[u8]) -> Option<Vec<(u32, u32)>> {
    let fc = get_u32(word_doc, FIB_FC_SPA_MOM)? as usize;
    let lcb = get_u32(word_doc, FIB_FC_SPA_MOM + 4)? as usize;
    let plc = table.get(fc..fc.saturating_add(lcb))?;
    if lcb < 4 + FSPA_LEN {
        return None;
    }
    let n = (lcb - 4) / (4 + FSPA_LEN);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let cp = get_u32(plc, i * 4)?;
        let spid = get_u32(plc, (n + 1) * 4 + i * FSPA_LEN)?;
        out.push((cp, spid));
    }
    Some(out)
}

/// The OfficeArt drawing: the picture offsets of the BStore, in BStore order,
/// and the picture each shape displays.
#[derive(Default)]
struct Drawing {
    /// `foDelay` of each BStore entry.
    blips: Vec<u32>,
    /// (spid, pib) per shape that displays a picture.
    pibs: Vec<(u32, u32)>,
}

impl Drawing {
    fn parse(table: &[u8], fc: usize, lcb: usize) -> Self {
        let mut drawing = Drawing::default();
        let end = fc.saturating_add(lcb).min(table.len());
        if fc >= end {
            return drawing;
        }
        let mut spid = None;
        let mut visit = |_ver_inst: u16, rec_type: u16, body: &[u8]| match rec_type {
            RT_FBSE => {
                if let Some(offset) = get_u32(body, 28) {
                    drawing.blips.push(offset);
                }
            }
            // `FSP` and `FOPT` are siblings inside one `SpContainer`, so a flat
            // walk sees the shape id immediately before its properties.
            RT_FSP => spid = get_u32(body, 0),
            RT_FOPT => {
                if let (Some(spid), Some(pib)) = (spid, fopt_prop(body, PROP_PIB)) {
                    drawing.pibs.push((spid, pib));
                }
            }
            _ => {}
        };
        walk(table, fc, end, 0, &mut visit);
        drawing
    }

    /// The BStore entry a `pib` names, as an offset into the stream that holds
    /// the picture bytes (`pib` is 1-based).
    fn blip_offset(&self, pib: u32) -> Option<usize> {
        let index = pib.checked_sub(1)? as usize;
        self.blips.get(index).map(|offset| *offset as usize)
    }

    fn pib_of(&self, spid: u32) -> Option<u32> {
        self.pibs.iter().find(|(id, _)| *id == spid).map(|(_, pib)| *pib)
    }
}

/// Visit every OfficeArt record in `data[start..end]`, descending into
/// containers. Single bytes that are not a record header are skipped: Word pads
/// between top-level containers of the drawing.
fn walk<F: FnMut(u16, u16, &[u8])>(data: &[u8], start: usize, end: usize, depth: usize, f: &mut F) {
    if depth > 16 {
        return;
    }
    let mut off = start;
    while off + 8 <= end {
        let Some((ver_inst, rec_type, body)) = record_at(&data[..end], off) else {
            off += 1;
            continue;
        };
        if rec_type & 0xF000 != 0xF000 {
            off += 1;
            continue;
        }
        let len = body.len();
        f(ver_inst, rec_type, body);
        if ver_inst & 0xF == 0xF {
            let inner = off + 8;
            walk(data, inner, inner + len, depth + 1, f);
        }
        off += 8 + len;
    }
}

/// The value of a simple `FOPT` property, or `None` when absent.
fn fopt_prop(body: &[u8], opid: u16) -> Option<u32> {
    // The record instance is the property count; each simple property is
    // (opid: u16, op: u32). Only the fixed-size part is scanned, so a complex
    // property's trailing data cannot be mistaken for another property.
    let mut p = 0usize;
    while p + 6 <= body.len() {
        let id = u16::from_le_bytes([body[p], body[p + 1]]);
        if id & 0x3FFF == opid {
            return Some(u32::from_le_bytes([body[p + 2], body[p + 3], body[p + 4], body[p + 5]]));
        }
        p += 6;
    }
    None
}

/// Decode the blip record an `FBSE.foDelay` points at. Word keeps those bytes
/// in the WordDocument stream; the Data stream is the fallback for documents
/// that delay into it instead.
fn blip_at<'a>(
    word_doc: &'a [u8],
    data: &'a [u8],
    offset: usize,
    max_bytes: usize,
) -> Option<Blip<'a>> {
    [word_doc, data].into_iter().find_map(|stream| {
        let (ver_inst, rec_type, body) = record_at(stream, offset)?;
        decode_blip(ver_inst, rec_type, body, max_bytes)
    })
}
