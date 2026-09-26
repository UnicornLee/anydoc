//! Convert a document to Markdown **keeping its embedded images**.
//!
//! The library's own serializer renders an embedded image as its alt text,
//! which is empty for most `.doc` pictures - formulas silently vanish. This
//! example walks the document model instead: every asset is written next to
//! the Markdown and referenced by path, so a picture keeps its place in the
//! text even with no alt text at all.
//!
//! `cargo run --example doc_with_images -- <file> [asset-dir-name]`
//!
//! The Markdown lands beside the source; assets go into a sibling directory
//! (`<stem>_files` by default) and are referenced relatively. Rendering is
//! deliberately simple - headings, paragraphs, lists, tables, quotes, code,
//! rules, math, links and notes - without the escaping subtleties of the
//! library's own serializer.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anydoc::model::{
    Block, CellSlot, Document, ImageSource, Inline, LinkTarget, List, Style, Table,
};
use anydoc::{ConvertError, Format};

const USAGE: &str = "usage: doc_with_images <file> [asset-dir-name]";

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("{USAGE}");
        return if args.is_empty() { ExitCode::FAILURE } else { ExitCode::SUCCESS };
    }
    let path = PathBuf::from(&args[0]);
    let dir_name = match args.get(1) {
        Some(name) => name.to_string_lossy().to_string(),
        None => format!("{}_files", path.file_stem().unwrap_or_default().to_string_lossy()),
    };
    match run(&path, &dir_name) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: &Path, dir_name: &str) -> Result<(), ConvertError> {
    let document = load_document(path)?;

    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let assets_dir = parent.join(dir_name);
    std::fs::create_dir_all(&assets_dir)?;

    let mut names = Vec::with_capacity(document.assets.len());
    let mut is_image = Vec::with_capacity(document.assets.len());
    for asset in &document.assets {
        let name = asset_name(&asset.media_type, asset.id.0);
        std::fs::write(assets_dir.join(&name), &asset.bytes)?;
        is_image.push(asset.media_type.starts_with("image/"));
        names.push(name);
    }

    let renderer = Renderer { dir: dir_name.to_string(), names, is_image };
    let mut markdown = String::new();
    renderer.blocks(&document.blocks, 0, &mut markdown);
    if !document.notes.is_empty() {
        markdown.push_str("---\n\n");
        for note in &document.notes {
            let mut body = String::new();
            renderer.blocks(&note.blocks, 0, &mut body);
            let body = body.trim().replace('\n', " ");
            let _ =
                std::fmt::Write::write_fmt(&mut markdown, format_args!("[^{}]: {body}\n", note.id));
        }
        markdown.push('\n');
    }

    let out = parent.join(format!("{stem}.md"));
    std::fs::write(&out, markdown.as_bytes())?;
    println!("{} -> {}", path.display(), out.display());
    eprintln!("  {} assets in {}", document.assets.len(), assets_dir.display());
    Ok(())
}

/// Parse the input file. A legacy binary `.doc` is read by the built-in
/// parser first: it decodes the `Equation Native` MTEF streams of embedded OLE
/// objects to LaTeX and resolves floating pictures through the `Plcspa` /
/// `DggInfo` drawing, which is exactly what a round-trip through Word yields
/// without needing an office suite installed.
///
/// Only when that parse fails is an office suite tried, as a structure
/// fallback: Microsoft Word round-trips faithfully, while LibreOffice works
/// headless on Linux but was observed to drop MathType OLE objects entirely
/// (verified on a real workbook, LO 25.x). Set `ANYDOC_OFFICE_DOC=1` to skip
/// straight to the office suite, for comparing the two routes.
fn load_document(path: &Path) -> Result<Document, ConvertError> {
    let bytes = std::fs::read(path)?;
    let format =
        Format::from_bytes(&bytes).or_else(|| Format::from_path(path)).ok_or_else(|| {
            ConvertError::Unsupported(format!(
                "unrecognized file content and extension: {}",
                path.display()
            ))
        })?;
    let native = anydoc::to_document(&bytes, format);
    if !matches!(format, Format::Doc) {
        return native;
    }
    if std::env::var_os("ANYDOC_OFFICE_DOC").is_some() {
        return match doc_via_office(path) {
            Some(document) => Ok(document),
            None => native,
        };
    }
    match native {
        Ok(document) => Ok(document),
        Err(e) => {
            eprintln!("note: built-in .doc parser failed ({e:#}); trying an office suite");
            // Nothing else worked: surface the parser's error, which describes
            // the document rather than a missing converter.
            doc_via_office(path).ok_or(e)
        }
    }
}

/// Round-trip a `.doc` through an installed office suite and parse the result.
fn doc_via_office(path: &Path) -> Option<Document> {
    #[cfg(windows)]
    if let Some(docx) = word_doc_to_docx(path) {
        eprintln!("note: .doc round-tripped through Microsoft Word");
        if let Ok(document) = anydoc::to_document(&docx, Format::Docx) {
            return Some(document);
        }
    }
    if let Some(docx) = libreoffice_doc_to_docx(path) {
        eprintln!(
            "note: .doc round-tripped through LibreOffice (MathType OLE objects may be dropped)"
        );
        if let Ok(document) = anydoc::to_document(&docx, Format::Docx) {
            return Some(document);
        }
    }
    None
}

/// Convert `.doc` to `.docx` through Microsoft Word COM automation, returning
/// the converted bytes. Returns `None` (with a diagnostic on stderr) when
/// Word is not installed or the conversion fails, so the caller can fall
/// back to the built-in legacy parser.
#[cfg(windows)]
fn word_doc_to_docx(path: &Path) -> Option<Vec<u8>> {
    use std::io::Write as _;
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos()
    );
    let tmp = std::env::temp_dir();
    let script = tmp.join(format!("anydoc-word-{unique}.ps1"));
    let dst = tmp.join(format!("anydoc-word-{unique}.docx"));

    // PowerShell reads a BOM-less file as ANSI, which mangles non-ASCII
    // paths - a UTF-8 BOM makes both Windows PowerShell 5.1 and pwsh 7+
    // decode the script (and its quoted paths) correctly.
    let ps_quote = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let body = format!(
        "$ErrorActionPreference = 'Stop'\n\
         $word = $null\n\
         try {{\n\
         \x20   $word = New-Object -ComObject Word.Application\n\
         \x20   $word.Visible = $false\n\
         \x20   $word.DisplayAlerts = 0\n\
         \x20   $doc = $word.Documents.Open('{src}', $false, $true)\n\
         \x20   $doc.SaveAs2('{dst}', 12)\n\
         \x20   $doc.Close($false)\n\
         }} finally {{\n\
         \x20   if ($word -ne $null) {{ $word.Quit() }}\n\
         }}\n",
        src = ps_quote(path),
        dst = ps_quote(&dst),
    );
    let mut file = std::fs::File::create(&script).ok()?;
    file.write_all(b"\xEF\xBB\xBF").ok()?;
    file.write_all(body.as_bytes()).ok()?;
    file.flush().ok()?;
    drop(file);

    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let bytes = match status {
        Ok(s) if s.success() => std::fs::read(&dst).ok(),
        _ => None,
    };
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_file(&dst);
    if bytes.is_none() {
        eprintln!("note: Word COM conversion failed for {}", path.display());
    }
    bytes
}

/// Convert `.doc` to `.docx` with a headless LibreOffice (`soffice
/// --convert-to docx`), returning the converted bytes. This is the
/// production path on Linux, and the fallback on Windows when Word is
/// absent. Returns `None` (with a diagnostic on stderr) when soffice is
/// not installed or the conversion fails.
fn libreoffice_doc_to_docx(path: &Path) -> Option<Vec<u8>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos()
    );
    let work = std::env::temp_dir().join(format!("anydoc-lo-{unique}"));
    let outdir = work.join("out");
    std::fs::create_dir_all(&outdir).ok()?;
    let dst = outdir.join(path.file_name()?).with_extension("docx");

    // A private user profile keeps concurrent runs from fighting over
    // soffice's single-instance lock, and works in containers that have
    // no writable $HOME.
    let profile =
        format!("file:///{}", work.join("profile").display().to_string().replace('\\', "/"));
    let status = std::process::Command::new("soffice")
        .arg(format!("-env:UserInstallation={profile}"))
        .args(["--headless", "--norestore", "--convert-to", "docx", "--outdir"])
        .arg(&outdir)
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let bytes = match status {
        Ok(s) if s.success() => std::fs::read(&dst).ok(),
        _ => None,
    };
    let _ = std::fs::remove_dir_all(&work);
    if bytes.is_none() {
        eprintln!("note: LibreOffice conversion failed for {}", path.display());
    }
    bytes
}

/// An asset's file name: its index, with an extension from its media type.
fn asset_name(media_type: &str, id: usize) -> String {
    let (_kind, subtype) = media_type.split_once('/').unwrap_or(("", ""));
    let subtype = if subtype == "jpeg" { "jpg" } else { subtype };
    let ext: String = subtype.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if ext.is_empty() { format!("{id}.bin") } else { format!("{id}.{ext}") }
}

/// Merge neighboring runs that carry the same style: a style toggle that
/// changes nothing would otherwise emit nested markers (`****6****`).
fn merge_text_runs(inlines: &[Inline]) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match (out.last_mut(), inline) {
            (
                Some(Inline::Text { text, style }),
                Inline::Text { text: next, style: next_style },
            ) if *style == *next_style => text.push_str(next),
            _ => out.push(inline.clone()),
        }
    }
    out
}

struct Renderer {
    /// Asset directory name, as the Markdown references it.
    dir: String,
    /// File name per asset index, for `Inline::Image` references.
    names: Vec<String>,
    /// Whether each asset is a picture. An OLE payload is not, and pointing
    /// an image at it only yields a permanently broken image.
    is_image: Vec<bool>,
}

impl Renderer {
    fn blocks(&self, blocks: &[Block], depth: usize, out: &mut String) {
        for block in blocks {
            match block {
                Block::Heading { level, content, .. } => {
                    out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
                    out.push(' ');
                    self.inlines(content, out);
                    out.push_str("\n\n");
                }
                Block::Paragraph(inlines) => {
                    let mut text = String::new();
                    self.inlines(inlines, &mut text);
                    let text = text.trim();
                    if !text.is_empty() {
                        out.push_str(text);
                        out.push_str("\n\n");
                    }
                }
                Block::List(list) => self.list(list, depth, out),
                Block::Table(table) => self.table(table, out),
                Block::BlockQuote(blocks) => {
                    let mut inner = String::new();
                    self.blocks(blocks, depth, &mut inner);
                    for line in inner.trim_end().lines() {
                        out.push_str("> ");
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push('\n');
                }
                Block::CodeBlock { lang, text } => {
                    out.push_str("```");
                    out.push_str(lang.as_deref().unwrap_or(""));
                    out.push('\n');
                    out.push_str(text.trim_end());
                    out.push_str("\n```\n\n");
                }
                Block::Rule => out.push_str("---\n\n"),
                Block::Math(tex) => {
                    out.push_str("$$\n");
                    out.push_str(tex.trim());
                    out.push_str("\n$$\n\n");
                }
            }
        }
    }

    fn list(&self, list: &List, depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth);
        for (i, item) in list.items.iter().enumerate() {
            let label = item
                .marker_label
                .clone()
                .unwrap_or_else(|| list.marker.label(list.start + i as u64));
            let mut body = String::new();
            self.blocks(&item.blocks, depth + 1, &mut body);
            let pad = " ".repeat(label.chars().count() + 1);
            let mut lines = body.trim_end().lines();
            out.push_str(&indent);
            out.push_str(&label);
            match lines.next() {
                Some(first) => {
                    out.push(' ');
                    out.push_str(first);
                }
                None => {}
            }
            out.push('\n');
            for line in lines {
                out.push_str(&indent);
                out.push_str(&pad);
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push('\n');
    }

    fn table(&self, table: &Table, out: &mut String) {
        let rows: Vec<Vec<String>> = table
            .grid
            .iter()
            .map(|row| {
                row.iter()
                    .map(|slot| match slot {
                        CellSlot::Origin(cell) => {
                            let mut text = String::new();
                            self.blocks(&cell.blocks, 0, &mut text);
                            // A cell is one line in Markdown: collapse the
                            // block breaks and keep `|` from breaking the row.
                            text.split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ")
                                .replace('|', "\\|")
                        }
                        // Swallowed by a span: GFM has no merged cells.
                        CellSlot::Covered { .. } => String::new(),
                    })
                    .collect()
            })
            .collect();
        let Some(width) = rows.iter().map(|r| r.len()).max().filter(|w| *w > 0) else {
            return;
        };
        for (i, row) in rows.iter().enumerate() {
            out.push('|');
            for col in 0..width {
                out.push(' ');
                out.push_str(row.get(col).map(String::as_str).unwrap_or(""));
                out.push_str(" |");
            }
            out.push('\n');
            if i + 1 == table.header_rows {
                out.push('|');
                for _ in 0..width {
                    out.push_str(" --- |");
                }
                out.push('\n');
            }
        }
        out.push('\n');
    }

    fn inlines(&self, inlines: &[Inline], out: &mut String) {
        for inline in &merge_text_runs(inlines) {
            match inline {
                Inline::Text { text, style } => self.text(text, *style, out),
                Inline::Link { content, target } => {
                    let mut label = String::new();
                    self.inlines(content, &mut label);
                    let (url, prefix) = match target {
                        LinkTarget::External(s) | LinkTarget::Relative(s) => (s.as_str(), ""),
                        LinkTarget::Anchor(id) => (id.as_str(), "#"),
                    };
                    if target.is_empty() || label.trim().is_empty() {
                        out.push_str(&label);
                    } else {
                        let _ = std::fmt::Write::write_fmt(
                            out,
                            format_args!("[{}]({prefix}{url})", label.trim()),
                        );
                    }
                }
                Inline::Image { alt, source } => self.image(alt, source, out),
                Inline::Anchor(id) => {
                    let _ = std::fmt::Write::write_fmt(out, format_args!("<a id=\"{id}\"></a>"));
                }
                Inline::NoteRef(id) => {
                    let _ = std::fmt::Write::write_fmt(out, format_args!("[^{id}]"));
                }
                Inline::LineBreak | Inline::PageBreak => out.push('\n'),
                Inline::Math(tex) => {
                    out.push('$');
                    out.push_str(tex.trim());
                    out.push('$');
                }
                Inline::Checkbox(checked) => out.push_str(if *checked { "[x]" } else { "[ ]" }),
            }
        }
    }

    fn image(&self, alt: &str, source: &ImageSource, out: &mut String) {
        match source {
            ImageSource::External(url) => {
                let _ = std::fmt::Write::write_fmt(out, format_args!("![{}]({url})", alt.trim()));
            }
            // The point of this example: an embedded image keeps its place
            // even when the source gave it no alt text.
            ImageSource::Asset(id) => {
                let name = self.names.get(id.0).map(String::as_str).unwrap_or("");
                let alt = alt.trim();
                let url = format!("{}/{name}", self.dir);
                if self.is_image.get(id.0).copied().unwrap_or(false) {
                    let _ = std::fmt::Write::write_fmt(out, format_args!("![{alt}]({url})"));
                } else {
                    // Not a picture - an OLE payload, say. Markdown cannot
                    // show it and prose should not carry a placeholder, so
                    // leave a comment: invisible when rendered, but the
                    // payload stays traceable in the source.
                    let label = if alt.is_empty() { "embedded object" } else { alt };
                    let _ =
                        std::fmt::Write::write_fmt(out, format_args!("<!-- {label}: {url} -->"));
                }
            }
            ImageSource::Unavailable => out.push_str(alt.trim()),
        }
    }

    fn text(&self, text: &str, style: Style, out: &mut String) {
        if text.is_empty() {
            return;
        }
        if style.code {
            out.push('`');
        }
        if style.bold {
            out.push_str("**");
        }
        if style.italic {
            out.push('*');
        }
        if style.strike {
            out.push_str("~~");
        }
        out.push_str(text);
        if style.strike {
            out.push_str("~~");
        }
        if style.italic {
            out.push('*');
        }
        if style.bold {
            out.push_str("**");
        }
        if style.code {
            out.push('`');
        }
    }
}
