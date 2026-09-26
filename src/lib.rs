//! anydoc converts documents to GitHub-Flavored Markdown.
//!
//! Recovery and skipped-content events are reported through the [`log`]
//! facade (debug/warn level); logging never changes conversion behavior and
//! its messages are not a stable API.

#![warn(missing_docs)]

pub mod model;

mod error;
mod formats;
mod package;
mod render;
mod shared;

pub use error::ConvertError;

use render::markdown::document_to_markdown;

use std::path::Path;

/// Input format. Selects the parser; container variants that share a parser
/// (docm, xlsm, ...) map onto these via [`Format::from_bytes`] or
/// [`Format::from_extension`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Binary Word 97-2003 (`.doc`).
    Doc,
    /// WordprocessingML (`.docx`, `.docm`), both Transitional and Strict.
    Docx,
    /// OpenDocument Text (`.odt`).
    Odt,
    /// Converted with [pdf-inspector], which emits Markdown directly:
    /// [`to_document`] is unsupported for PDFs. Scanned or image-only pages
    /// need OCR, which anydoc does not do: the document errors with
    /// [`ConvertError::NeedsOcr`] naming them.
    ///
    /// [pdf-inspector]: https://github.com/firecrawl/pdf-inspector
    Pdf,
    /// Binary PowerPoint 97-2003 (`.ppt`, `.pps`, `.pot`).
    Ppt,
    /// PresentationML (`.pptx`, `.pptm`, `.ppsx`, `.ppsm`).
    Pptx,
    /// Rich Text Format (`.rtf`).
    Rtf,
    /// EPUB 2 and 3 (`.epub`).
    Epub,
    /// Excel workbooks: `.xlsx`, `.xlsm`, binary `.xlsb`, and legacy
    /// OLE-based `.xls`.
    Excel,
    /// OpenDocument Spreadsheet (`.ods`).
    Ods,
    /// OpenDocument Presentation (`.odp`).
    Odp,
    /// Delimiter-separated text (`.csv`). Carries no signature, so it has to
    /// be named rather than detected.
    Csv,
}

impl Format {
    /// Detect the format from the content itself: the signature and identity
    /// each container specification designates (PDF header, RTF open group,
    /// OLE stream names, ZIP package mimetype/content types). Plain-text
    /// formats (CSV) carry no signature and return `None`; so does anything
    /// unrecognized.
    pub fn from_bytes(bytes: &[u8]) -> Option<Format> {
        formats::detect::from_bytes(bytes)
    }

    /// The format a bare extension names (no leading dot), matched
    /// case-insensitively. `None` for anything unrecognized.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "doc" => Format::Doc,
            "docx" | "docm" => Format::Docx,
            "odt" => Format::Odt,
            "pdf" => Format::Pdf,
            "pptx" | "pptm" | "ppsx" | "ppsm" => Format::Pptx,
            "ppt" | "pps" | "pot" => Format::Ppt,
            "rtf" => Format::Rtf,
            "epub" => Format::Epub,
            "xlsx" | "xlsm" | "xlsb" | "xls" => Format::Excel,
            "ods" => Format::Ods,
            "odp" => Format::Odp,
            "csv" => Format::Csv,
            _ => return None,
        })
    }

    /// The format a path's extension names. `None` when the path has no
    /// extension or names nothing recognized.
    pub fn from_path(path: &Path) -> Option<Format> {
        path.extension().and_then(|e| e.to_str()).and_then(Format::from_extension)
    }
}

/// Convert a document file to Markdown. The format is detected from the
/// file content ([`Format::from_bytes`]); the extension is the fallback for
/// signature-less formats (CSV) and unrecognizable containers.
pub fn to_markdown(path: impl AsRef<Path>) -> Result<String, ConvertError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let Some(format) = Format::from_bytes(&bytes).or_else(|| Format::from_path(path)) else {
        return Err(ConvertError::Unsupported(format!(
            "unrecognized file content and extension: {}",
            path.display()
        )));
    };
    to_markdown_bytes(&bytes, format)
}

/// Convert an in-memory document to Markdown. Pass a [`Format`] to select the
/// parser, or `None` to detect it from the content ([`Format::from_bytes`]),
/// which signature-less formats (CSV) have to name explicitly.
pub fn to_markdown_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<String, ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    // PDFs convert to Markdown directly (pdf-inspector) without passing
    // through the document model.
    if format == Format::Pdf {
        return formats::pdf::to_markdown(bytes);
    }
    Ok(document_to_markdown(&to_document(bytes, format)?))
}

/// Convert a document file to one Markdown string per page. The format is
/// detected from the file content ([`Format::from_bytes`]); the extension is
/// the fallback for signature-less formats (CSV) and unrecognizable
/// containers.
///
/// Pages follow the page boundaries the source records — explicit page
/// breaks and the layout engine's marks — so they approximate, but do not
/// reproduce, the pagination of a word processor. Documents with no page
/// marks come back as a single page; the result is never empty.
///
/// Sources that carry layout-engine page marks (Word's
/// `lastRenderedPageBreak`, ODT `soft-page-break`) also gain a line break
/// there in [`to_markdown`] output: the mark replaces the word-joining
/// nothing older conversions produced across the boundary.
pub fn to_pages(path: impl AsRef<Path>) -> Result<Vec<String>, ConvertError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    let Some(format) = Format::from_bytes(&bytes).or_else(|| Format::from_path(path)) else {
        return Err(ConvertError::Unsupported(format!(
            "unrecognized file content and extension: {}",
            path.display()
        )));
    };
    to_pages_bytes(&bytes, format)
}

/// Convert an in-memory document to one Markdown string per page. Pass a
/// [`Format`] to select the parser, or `None` to detect it from the content
/// ([`Format::from_bytes`]), which signature-less formats (CSV) have to name
/// explicitly. See [`to_pages`] for what a page is.
pub fn to_pages_bytes(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<Vec<String>, ConvertError> {
    let format = resolve_format(bytes, format.into())?;
    // PDFs paginate natively: pdf-inspector extracts each physical page.
    if format == Format::Pdf {
        return formats::pdf::to_pages(bytes);
    }
    let doc = to_document(bytes, format)?;
    Ok(render::paginate::paginate(&doc).iter().map(document_to_markdown).collect())
}

/// Parse an in-memory document into the document model. Pass a [`Format`] to
/// select the parser, or `None` to detect it from the content.
///
/// Unsupported for [`Format::Pdf`]: PDF conversion produces Markdown
/// directly and has no document-model form; use [`to_markdown_bytes`].
pub fn to_document(
    bytes: &[u8],
    format: impl Into<Option<Format>>,
) -> Result<model::Document, ConvertError> {
    formats::parse(bytes, resolve_format(bytes, format.into())?)
}

fn resolve_format(bytes: &[u8], format: Option<Format>) -> Result<Format, ConvertError> {
    format.or_else(|| Format::from_bytes(bytes)).ok_or_else(|| {
        ConvertError::Unsupported("unrecognized file content: name the format explicitly".into())
    })
}
