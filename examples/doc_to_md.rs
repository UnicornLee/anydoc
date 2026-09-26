//! Convert documents to Markdown next to the original file:
//! `cargo run --example doc_to_md -- <file>...`
//!
//! Each output sits in the same directory as its input, with the same name
//! and a `.md` extension. Existing files are overwritten. The format comes
//! from the file content, with the extension as the fallback.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anydoc::{ConvertError, Format};

const USAGE: &str = "usage: doc_to_md <file>...";

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("{USAGE}");
        return if args.is_empty() { ExitCode::FAILURE } else { ExitCode::SUCCESS };
    }

    // One bad document does not stop the rest.
    let mut failed = false;
    for arg in &args {
        match convert(Path::new(arg)) {
            Ok(out) => println!("{} -> {}", Path::new(arg).display(), out.display()),
            Err(e) => {
                failed = true;
                eprintln!("error: {}: {e:#}", Path::new(arg).display());
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn convert(path: &Path) -> Result<PathBuf, ConvertError> {
    let bytes = std::fs::read(path)?;
    let format =
        Format::from_bytes(&bytes).or_else(|| Format::from_path(path)).ok_or_else(|| {
            ConvertError::Unsupported(format!(
                "unrecognized file content and extension: {}",
                path.display()
            ))
        })?;

    let start = Instant::now();
    let markdown = anydoc::to_markdown_bytes(&bytes, format)?;
    let elapsed = start.elapsed();

    // Same directory, same name, `.md` instead of the original extension.
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let out = path.parent().unwrap_or_else(|| Path::new("")).join(format!("{stem}.md"));
    std::fs::write(&out, markdown.as_bytes())?;

    eprintln!("  {} in {}, {} bytes", format_name(format), millis(elapsed), markdown.len());
    Ok(out)
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Doc => "doc",
        Format::Docx => "docx",
        Format::Odt => "odt",
        Format::Pdf => "pdf",
        Format::Ppt => "ppt",
        Format::Pptx => "pptx",
        Format::Rtf => "rtf",
        Format::Epub => "epub",
        Format::Excel => "xlsx",
        Format::Ods => "ods",
        Format::Odp => "odp",
        Format::Csv => "csv",
    }
}

fn millis(elapsed: std::time::Duration) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms < 10.0 { format!("{ms:.2}ms") } else { format!("{ms:.0}ms") }
}
