//! Render DrawingML charts, SmartArt and drawn shapes to PNG.
//!
//! WordprocessingML keeps charts (`c:chart`), SmartArt (`dgm:relIds`) and
//! drawn shapes (`wps:wsp`) as vector markup, not bitmaps. Markdown cannot
//! show any of them, and no cross-platform Rust crate can play them back
//! faithfully, so rendering is delegated to LibreOffice.
//!
//! LibreOffice cannot rasterize one drawing out of a document, so each
//! drawing is isolated first: the original package is re-zipped with a
//! `word/document.xml` whose body holds just that drawing (or one group of
//! floating shapes that belong together). Rendering that one-drawing
//! document yields a page whose only content is the drawing; cropping the
//! white margin leaves the drawing alone.
//!
//! LibreOffice is optional. When no binary is found — or anything fails —
//! this returns `None` and callers keep their text-only fallback.

use std::collections::HashMap;
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// What one rendered drawing contributes to the output.
#[derive(Clone)]
pub enum DiagramSlot {
    /// This drawing owns a rendered PNG; emit it.
    Primary(Vec<u8>),
    /// This drawing was merged into an earlier one's image; skip it so the
    /// same picture is not emitted once per shape.
    Merged,
}

/// Render every chart, SmartArt and drawn shape in a docx to PNG, keyed by
/// the drawing's `wp:docPr/@id` so callers can look a drawing up while they
/// walk the document.
///
/// Returns `None` when LibreOffice is unavailable, the package cannot be
/// rewritten, or nothing rendered — callers keep their text-only path.
pub fn render_docx_diagrams(docx: &[u8]) -> Option<HashMap<String, DiagramSlot>> {
    let source = read_entry(docx, "word/document.xml")?;
    let xml = String::from_utf8(source).ok()?;
    let groups = drawing_groups(&xml);
    // Checked before probing for LibreOffice: a document with no charts or
    // shapes must not pay for a process that would render nothing.
    if groups.is_empty() {
        return None;
    }
    let soffice = soffice()?;
    let sect_pr = section_properties(&xml);
    let mut out = HashMap::new();
    render_groups(soffice, docx, &xml, &groups, &sect_pr, &mut out);
    (!out.is_empty()).then_some(out)
}

// --- LibreOffice discovery ------------------------------------------------

/// Absolute path candidates outside `PATH`, because LibreOffice installs
/// there on every desktop OS but never registers itself on `PATH` on
/// Windows or macOS.
const SOFFICE_CANDIDATES: &[&str] = &[
    "C:\\Program Files\\LibreOffice\\program\\soffice.exe",
    "C:\\Program Files (x86)\\LibreOffice\\program\\soffice.exe",
    "/Applications/LibreOffice.app/Contents/MacOS/soffice",
    "/usr/bin/soffice",
    "/usr/local/bin/soffice",
    "/usr/lib/libreoffice/program/soffice",
    "/opt/libreoffice/program/soffice",
];

/// Locate a working `soffice`, probing `PATH` first and then the usual
/// install locations. The result is cached: a missing LibreOffice must not
/// pay the process spawn on every document.
fn soffice() -> Option<&'static PathBuf> {
    static ONCE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        for name in ["soffice", "libreoffice", "soffice.com"] {
            if probe(Path::new(name)) {
                return Some(PathBuf::from(name));
            }
        }
        for path in SOFFICE_CANDIDATES {
            let path = PathBuf::from(path);
            if path.is_file() && probe(&path) {
                return Some(path);
            }
        }
        None
    })
    .as_ref()
}

/// `--version` exits without touching a display or a user profile, so it is
/// a safe availability probe.
fn probe(program: &Path) -> bool {
    Command::new(program)
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

// --- Finding the drawings -------------------------------------------------

/// One unit of rendering: the drawings that share a paragraph, in document
/// order. Floating shapes are anchored to the same paragraph as each other,
/// so grouping by paragraph is what makes a multi-shape flow chart come out
/// as a single picture.
struct Group {
    /// `wp:docPr/@id` of each drawing in the group, in document order.
    ids: Vec<String>,
    /// The raw `<w:p>...</w:p>` markup to copy into the variant document.
    paragraph: String,
}

/// Which drawings in a `<w:drawing>` block need rendering.
fn drawing_kind(block: &str) -> Kind {
    if block.contains("<c:chart") {
        Kind::Render
    } else if block.contains("<dgm:relIds") {
        Kind::Render
    } else if block.contains("<wps:") || block.contains("<wpg:") {
        // A drawn shape, unless it is only carrying a bitmap fill.
        if block.contains("<a:blip") { Kind::Skip } else { Kind::Render }
    } else {
        Kind::Skip
    }
}

enum Kind {
    Render,
    Skip,
}

/// `wp:docPr/@id` of a drawing block, the stable key a caller can match.
fn doc_pr_id(block: &str) -> Option<String> {
    let start = block.find("<wp:docPr")?;
    let tag_end = block[start..].find('>')? + start;
    let tag = &block[start..tag_end];
    let id = tag.find("id=\"")? + 4;
    let end = tag[id..].find('"')? + id;
    Some(tag[id..end].to_string())
}

/// Collect the paragraph groups that hold a renderable drawing, in document
/// order. Paragraphs never nest in WordprocessingML, so scanning for
/// `<w:p` .. `</w:p>` is enough and keeps the original bytes intact.
fn drawing_groups(xml: &str) -> Vec<Group> {
    let mut groups = Vec::new();
    let mut pos = 0;
    while let Some(rel) = xml[pos..].find("<w:p") {
        let start = pos + rel;
        let after = xml.get(start + 4..start + 5).unwrap_or("");
        if after != ">" && after != " " {
            pos = start + 4;
            continue;
        }
        let Some(end_rel) = xml[start..].find("</w:p>") else { break };
        let end = start + end_rel + "</w:p>".len();
        pos = end;

        let paragraph = &xml[start..end];
        let mut ids = Vec::new();
        let mut scan = 0;
        while let Some(drel) = paragraph[scan..].find("<w:drawing") {
            let dstart = scan + drel;
            let Some(dend_rel) = paragraph[dstart..].find("</w:drawing>") else { break };
            let dend = dstart + dend_rel + "</w:drawing>".len();
            let block = &paragraph[dstart..dend];
            scan = dend;
            if matches!(drawing_kind(block), Kind::Render) {
                if let Some(id) = doc_pr_id(block) {
                    ids.push(id);
                }
            }
        }
        if !ids.is_empty() {
            groups.push(Group { ids, paragraph: paragraph.to_string() });
        }
    }
    groups
}

/// The document's section properties, copied into every variant so page
/// geometry (and therefore the drawing's size) matches the original.
fn section_properties(xml: &str) -> String {
    match xml.rfind("<w:sectPr") {
        Some(start) => match xml[start..].find("</w:sectPr>") {
            Some(rel) => xml[start..start + rel + "</w:sectPr>".len()].to_string(),
            None => match xml[start..].find("/>") {
                Some(rel) => xml[start..start + rel + 2].to_string(),
                None => String::new(),
            },
        },
        None => String::new(),
    }
}

// --- Rendering ------------------------------------------------------------

fn render_groups(
    soffice: &Path,
    docx: &[u8],
    xml: &str,
    groups: &[Group],
    sect_pr: &str,
    out: &mut HashMap<String, DiagramSlot>,
) {
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let work = std::env::temp_dir().join(format!("anydoc-diagram-{pid}-{counter}"));
    // A crashed earlier run may have left this directory behind.
    let _ = std::fs::remove_dir_all(&work);
    if std::fs::create_dir_all(&work).is_err() {
        return;
    }

    let mut inputs = Vec::with_capacity(groups.len());
    for (i, group) in groups.iter().enumerate() {
        let Some(bytes) = build_variant(docx, xml, &group.paragraph, sect_pr) else {
            continue;
        };
        let path = work.join(format!("d{i}.docx"));
        if std::fs::write(&path, bytes).is_err() {
            continue;
        }
        inputs.push(path);
    }
    if inputs.is_empty() {
        cleanup(&work);
        return;
    }

    // One LibreOffice start-up for the whole document: every variant is
    // passed in a single invocation.
    let profile = work.join("profile");
    let profile = format!("file:///{}", profile.display().to_string().replace('\\', "/"));
    let outdir = work.join("out");
    if std::fs::create_dir_all(&outdir).is_err() {
        cleanup(&work);
        return;
    }
    let status = Command::new(soffice)
        .arg(format!("-env:UserInstallation={profile}"))
        .args(["--headless", "--norestore", "--convert-to", "png", "--outdir"])
        .arg(&outdir)
        .args(&inputs)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        log::warn!("LibreOffice diagram rendering failed");
        cleanup(&work);
        return;
    }

    for (i, group) in groups.iter().enumerate() {
        let png = outdir.join(format!("d{i}.png"));
        let Some(rendered) = std::fs::read(&png).ok().and_then(|b| crop_white(&b)) else {
            continue;
        };
        for (n, id) in group.ids.iter().enumerate() {
            // The first drawing of a group carries the picture; the rest are
            // shapes that were already drawn into it.
            let slot =
                if n == 0 { DiagramSlot::Primary(rendered.clone()) } else { DiagramSlot::Merged };
            out.insert(id.clone(), slot);
        }
    }
    cleanup(&work);
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn cleanup(work: &Path) {
    let _ = std::fs::remove_dir_all(work);
}

/// Copy the package with a `document.xml` whose body is just `paragraph`.
/// Every other part (styles, theme, charts, diagram data) is kept, because
/// the drawing's appearance and its relationship targets depend on them.
fn build_variant(docx: &[u8], xml: &str, paragraph: &str, sect_pr: &str) -> Option<Vec<u8>> {
    let body = format!("<w:body>{paragraph}{sect_pr}</w:body>");
    let rewritten = replace_body(xml, &body)?;

    let mut src = zip::ZipArchive::new(Cursor::new(docx)).ok()?;
    let mut buf = Vec::with_capacity(docx.len());
    let mut dst = zip::ZipWriter::new(Cursor::new(&mut buf));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for i in 0..src.len() {
        let mut entry = src.by_index(i).ok()?;
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).ok()?;
        drop(entry);
        let payload = if name == "word/document.xml" { rewritten.as_bytes() } else { &bytes };
        dst.start_file(name, options).ok()?;
        dst.write_all(payload).ok()?;
    }
    let cursor = dst.finish().ok()?;
    drop(cursor);
    Some(buf)
}

/// Swap the document body for `body`, keeping the surrounding XML prolog and
/// root element attributes intact.
fn replace_body(xml: &str, body: &str) -> Option<String> {
    let start = xml.find("<w:body")?;
    let open_end = xml[start..].find('>')? + start + 1;
    let close = xml.find("</w:body>")?;
    if close < open_end {
        return None;
    }
    Some(format!("{}{}{}", &xml[..start], body, &xml[close + "</w:body>".len()..]))
}

/// Trim the white page margin LibreOffice leaves around the drawing.
fn crop_white(png: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(png).ok()?;
    let rgba = image.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            let p = rgba.get_pixel(x as u32, y as u32).0;
            // Anything meaningfully darker than white counts as content.
            if p[0] < 240 || p[1] < 240 || p[2] < 240 {
                if x < min_x {
                    min_x = x;
                }
                if y < min_y {
                    min_y = y;
                }
                if x > max_x {
                    max_x = x;
                }
                if y > max_y {
                    max_y = y;
                }
            }
        }
    }
    if max_x < min_x || max_y < min_y {
        return None;
    }
    // A small margin keeps strokes that touch the edge from being clipped.
    let pad_x = ((max_x - min_x + 1) / 50).max(2).min(min_x.min(w - 1 - max_x));
    let pad_y = ((max_y - min_y + 1) / 50).max(2).min(min_y.min(h - 1 - max_y));
    let x0 = min_x - pad_x;
    let y0 = min_y - pad_y;
    let cw = (max_x - min_x + 1) + pad_x * 2;
    let ch = (max_y - min_y + 1) + pad_y * 2;

    let cropped = image::imageops::crop_imm(&rgba, x0 as u32, y0 as u32, cw as u32, ch as u32);
    let mut out = Cursor::new(Vec::new());
    cropped.to_image().write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Read one entry out of a zip package.
fn read_entry(zip_bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).ok()?;
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawing(id: &str, inner: &str) -> String {
        format!("<w:drawing><wp:inline><wp:docPr id=\"{id}\"/>{inner}</wp:inline></w:drawing>")
    }

    const CHART: &str = "<a:graphic><a:graphicData><c:chart r:id=\"rId1\"/></a:graphicData></a:graphic>";
    const SMART_ART: &str =
        "<a:graphic><a:graphicData><dgm:relIds r:dm=\"rId2\"/></a:graphicData></a:graphic>";
    const SHAPE: &str = "<a:graphic><a:graphicData><wps:wsp/></a:graphicData></a:graphic>";
    const BITMAP: &str = "<a:graphic><a:graphicData><a:blip r:embed=\"rId3\"/></a:graphicData></a:graphic>";

    #[test]
    fn charts_and_smartart_are_rendered() {
        let xml = format!(
            "<w:document><w:body><w:p><w:r>{}</w:r></w:p><w:p><w:r>{}</w:r></w:p></w:body></w:document>",
            drawing("10", CHART),
            drawing("20", SMART_ART)
        );
        let groups = drawing_groups(&xml);
        assert_eq!(groups.len(), 2, "one group per drawing paragraph");
        assert_eq!(groups[0].ids, ["10"]);
        assert_eq!(groups[1].ids, ["20"]);
    }

    #[test]
    fn bitmaps_are_left_to_the_image_path() {
        let xml =
            format!("<w:document><w:body><w:p><w:r>{}</w:r></w:p></w:body></w:document>", drawing("30", BITMAP));
        assert!(drawing_groups(&xml).is_empty(), "a bitmap has its own asset path");
    }

    /// Floating shapes are anchored to the paragraph they float around, so a
    /// multi-shape flow chart must come out as one group — that is what makes
    /// it a single picture instead of one picture per shape.
    #[test]
    fn shapes_sharing_a_paragraph_form_one_group() {
        let xml = format!(
            "<w:document><w:body><w:p><w:r>{}{}{}</w:r></w:p></w:body></w:document>",
            drawing("40", SHAPE),
            drawing("41", SHAPE),
            drawing("42", SHAPE)
        );
        let groups = drawing_groups(&xml);
        assert_eq!(groups.len(), 1, "three anchored shapes are one picture");
        assert_eq!(groups[0].ids, ["40", "41", "42"]);
    }

    #[test]
    fn body_replacement_keeps_the_surrounding_document() {
        let xml = "<w:document><w:body><w:p/></w:body></w:document>";
        let out = replace_body(xml, "<w:body><w:p>new</w:p></w:body>").expect("body present");
        assert_eq!(out, "<w:document><w:body><w:p>new</w:p></w:body></w:document>");
    }

    #[test]
    fn section_properties_are_carried_over() {
        let xml = "<w:body><w:p/><w:sectPr><w:pgSz w:w=\"11906\"/></w:sectPr></w:body>";
        assert_eq!(section_properties(xml), "<w:sectPr><w:pgSz w:w=\"11906\"/></w:sectPr>");
    }
}
