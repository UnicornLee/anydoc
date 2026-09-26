//! Debug: count PageBreak inlines in a parsed document.
//!
//! Usage: cargo run --example dbg_pagebreaks -- <input-file>

use std::path::PathBuf;

fn count_inlines(inlines: &[anydoc::model::Inline]) -> usize {
    inlines
        .iter()
        .map(|inline| match inline {
            anydoc::model::Inline::PageBreak => 1,
            anydoc::model::Inline::Link { content, .. } => count_inlines(content),
            _ => 0,
        })
        .sum()
}

fn count_blocks(blocks: &[anydoc::model::Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            anydoc::model::Block::Paragraph(i) | anydoc::model::Block::Heading { content: i, .. } => {
                count_inlines(i)
            }
            anydoc::model::Block::List(list) => {
                list.items.iter().map(|item| count_blocks(&item.blocks)).sum()
            }
            anydoc::model::Block::Table(table) => table
                .grid
                .iter()
                .flatten()
                .map(|slot| match slot {
                    anydoc::model::CellSlot::Origin(cell) => count_blocks(&cell.blocks),
                    anydoc::model::CellSlot::Covered { .. } => 0,
                })
                .sum(),
            anydoc::model::Block::BlockQuote(inner) => count_blocks(inner),
            anydoc::model::Block::CodeBlock { .. } | anydoc::model::Block::Rule
            | anydoc::model::Block::Math(_) => 0,
        })
        .sum()
}

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("usage: dbg_pagebreaks <file>"));
    let bytes = std::fs::read(&path).unwrap();
    let doc = anydoc::to_document(&bytes, None).unwrap();
    let breaks = count_blocks(&doc.blocks);
    println!("blocks: {}, notes: {}, PageBreak inlines: {breaks}", doc.blocks.len(), doc.notes.len());
}
