//! PDF via [pdf-inspector]: classification plus direct Markdown extraction.
//!
//! Unlike the other frontends, pdf-inspector emits Markdown itself, so PDFs
//! bypass the document model and the shared GFM writer. OCR is out of scope
//! here: a document with scanned or image-only pages errors naming them,
//! whether that is every page or one of a hundred, because output missing
//! those pages would read as complete.
//!
//! [pdf-inspector]: https://github.com/firecrawl/pdf-inspector

use crate::error::ConvertError;
use pdf_inspector::PdfError;

pub fn to_markdown(bytes: &[u8]) -> Result<String, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    if !result.pages_needing_ocr.is_empty() {
        // Detection samples content streams and over-reports short or
        // image-heavy text pages; extraction knows which of them yielded none.
        let flagged: Vec<u32> = result.pages_needing_ocr.iter().map(|page| page - 1).collect();
        let pages = pdf_inspector::extract_pages_markdown_mem(bytes, Some(&flagged))
            .map_err(map_error)?
            .pages_needing_ocr;
        if !pages.is_empty() {
            return Err(ConvertError::NeedsOcr { pages, page_count: result.page_count });
        }
    }
    if result.has_encoding_issues {
        log::warn!("broken font encodings detected; extracted text may be garbled");
    }
    match result.markdown {
        Some(mut markdown) if !markdown.trim().is_empty() => {
            if !markdown.ends_with('\n') {
                markdown.push('\n');
            }
            Ok(markdown)
        }
        _ => Err(ConvertError::Unsupported(format!(
            "PDF has no extractable text ({:?}, {} pages)",
            result.pdf_type, result.page_count
        ))),
    }
}

/// Convert a PDF to one Markdown string per physical page. Detection
/// over-reports OCR-needing pages, so a flagged page only errors once
/// extraction confirms it yielded no text — the same confirmation
/// [`to_markdown`] performs. Blank physical pages are dropped.
pub fn to_pages(bytes: &[u8]) -> Result<Vec<String>, ConvertError> {
    let result = pdf_inspector::process_pdf_mem(bytes).map_err(map_error)?;
    if result.has_encoding_issues {
        log::warn!("broken font encodings detected; extracted text may be garbled");
    }
    let extracted = pdf_inspector::extract_pages_markdown_mem(bytes, None).map_err(map_error)?;
    // 1-indexed pages, matching ConvertError::NeedsOcr.
    let ocr: Vec<u32> = extracted
        .pages
        .iter()
        .filter(|page| page.needs_ocr && page.markdown.trim().is_empty())
        .map(|page| page.page + 1)
        .collect();
    if !ocr.is_empty() {
        return Err(ConvertError::NeedsOcr { pages: ocr, page_count: result.page_count });
    }
    let mut pages: Vec<String> = extracted
        .pages
        .into_iter()
        .map(|page| page.markdown)
        .filter(|markdown| !markdown.trim().is_empty())
        .collect();
    if pages.is_empty() {
        return Err(ConvertError::Unsupported(format!(
            "PDF has no extractable text ({:?}, {} pages)",
            result.pdf_type, result.page_count
        )));
    }
    for markdown in &mut pages {
        if !markdown.ends_with('\n') {
            markdown.push('\n');
        }
    }
    Ok(pages)
}

fn map_error(e: PdfError) -> ConvertError {
    match e {
        PdfError::Encrypted => ConvertError::Encrypted,
        PdfError::Io(e) => ConvertError::Io(e),
        PdfError::NotAPdf(detail) => ConvertError::malformed(format!("not a PDF: {detail}")),
        PdfError::InvalidStructure => ConvertError::malformed("invalid PDF structure"),
        PdfError::Parse(detail) => ConvertError::malformed(detail),
    }
}
