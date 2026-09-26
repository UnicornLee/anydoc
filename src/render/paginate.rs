//! Splitting a document into pages at its page-break marks.
//!
//! Every frontend records the page boundaries its format knows about
//! (explicit breaks, layout-engine marks) as [`Inline::PageBreak`]; this
//! pass cuts the block tree there and derives the per-page notes and
//! assets, so each page is a self-contained [`Document`] the normal
//! Markdown renderer can serialize on its own.

use crate::model::{
    AssetId, Block, CellSlot, Document, ImageSource, Inline, List, ListItem, MarkerKind, Note,
};
use std::collections::HashSet;

/// Split `doc` into per-page documents at every [`Inline::PageBreak`].
///
/// A document without page breaks comes back as a single page. Pages that
/// hold no visible content — breaks at the very start or end, or several in
/// a row — are dropped, so the result is never empty. Tables are atomic: a
/// page break inside a cell cannot split the table and is dropped.
pub fn paginate(doc: &Document) -> Vec<Document> {
    let mut pages: Vec<Document> = Vec::new();
    for blocks in split_into_pages(&doc.blocks) {
        if is_blank_page(&blocks) {
            continue;
        }
        let mut page = Document { blocks, notes: Vec::new(), assets: doc.assets.clone() };
        page.notes = page_notes(&page.blocks, &doc.notes);
        retain_page_assets(&mut page);
        pages.push(page);
    }
    if pages.is_empty() {
        pages.push(Document::default());
    }
    pages
}

/// Cut a block sequence into pages. The first page is always present, even
/// when empty (blank pages are filtered by the caller).
fn split_into_pages(blocks: &[Block]) -> Vec<Vec<Block>> {
    let mut pages: Vec<Vec<Block>> = vec![Vec::new()];
    for block in blocks {
        distribute(block, &mut pages);
    }
    pages
}

/// Push `block` onto the current page, splitting at the page breaks its
/// content carries and appending fresh pages to `pages` as needed.
fn distribute(block: &Block, pages: &mut Vec<Vec<Block>>) {
    match block {
        Block::Paragraph(inlines) => {
            for (idx, seg) in split_inlines(inlines).into_iter().enumerate() {
                if idx > 0 {
                    pages.push(Vec::new());
                }
                if !crate::model::inlines_are_empty(&seg) {
                    pages.last_mut().expect("page exists").push(Block::Paragraph(seg));
                }
            }
        }
        Block::Heading { level, anchor, content } => {
            // The anchor marks the heading's start: it goes to the first
            // segment that survives, even when a leading break moved that
            // segment to a later page.
            let mut anchor = anchor.clone();
            for (idx, seg) in split_inlines(content).into_iter().enumerate() {
                if idx > 0 {
                    pages.push(Vec::new());
                }
                if !crate::model::inlines_are_empty(&seg) {
                    pages.last_mut().expect("page exists").push(Block::Heading {
                        level: *level,
                        anchor: anchor.take(),
                        content: seg,
                    });
                }
            }
        }
        Block::List(list) => distribute_list(list, pages),
        Block::BlockQuote(inner) => {
            for (idx, seg) in split_into_pages(inner).into_iter().enumerate() {
                if idx > 0 {
                    pages.push(Vec::new());
                }
                if !is_blank_page(&seg) {
                    pages.last_mut().expect("page exists").push(Block::BlockQuote(seg));
                }
            }
        }
        Block::Table(table) => {
            if table_has_page_break(table) {
                // Tables are atomic: a break inside a cell cannot split the
                // grid, so it is dropped (like every break inside a table
                // cell in every other consumer).
                log::debug!("page break inside a table cell dropped");
            }
            pages.last_mut().expect("page exists").push(block.clone());
        }
        Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {
            pages.last_mut().expect("page exists").push(block.clone());
        }
    }
}

/// List items distribute across pages one by one: an item cut by a break
/// ends the current page's list, later items continue on the last page.
/// Ordering was resolved to ordinals during parsing, so a continued list
/// just restarts at the first pending item's ordinal.
fn distribute_list(list: &List, pages: &mut Vec<Vec<Block>>) {
    let mut pending: Vec<ListItem> = Vec::new();
    let mut pending_start = list.start;
    for (idx, item) in list.items.iter().enumerate() {
        let segs = split_into_pages(&item.blocks);
        for (seg_idx, seg) in segs.into_iter().enumerate() {
            if seg_idx > 0 {
                flush_list(pages, list.marker, &mut pending, &mut pending_start);
                pages.push(Vec::new());
            }
            if !is_blank_page(&seg) {
                if pending.is_empty() {
                    pending_start = list.start.saturating_add(idx as u64);
                }
                pending.push(ListItem { blocks: seg, marker_label: item.marker_label.clone() });
            }
        }
    }
    flush_list(pages, list.marker, &mut pending, &mut pending_start);
}

fn flush_list(
    pages: &mut Vec<Vec<Block>>,
    marker: MarkerKind,
    pending: &mut Vec<ListItem>,
    pending_start: &mut u64,
) {
    if pending.is_empty() {
        return;
    }
    let list = Block::List(List {
        marker,
        start: *pending_start,
        items: std::mem::take(pending),
    });
    pages.last_mut().expect("page exists").push(list);
}

/// Cut an inline sequence at its page breaks. A break inside a link splits
/// the link, cloning its target so each page keeps a working link.
fn split_inlines(inlines: &[Inline]) -> Vec<Vec<Inline>> {
    let mut pages: Vec<Vec<Inline>> = vec![Vec::new()];
    for inline in inlines {
        match inline {
            Inline::PageBreak => pages.push(Vec::new()),
            Inline::Link { content, target } if inlines_have_page_break(content) => {
                let segs = split_inlines(content);
                let cur = pages.last_mut().expect("page exists");
                // An empty first segment would render as a bare `[url](url)`;
                // the break already moved its content to the next page.
                if !segs[0].is_empty() {
                    cur.push(Inline::Link { content: segs[0].clone(), target: target.clone() });
                }
                for seg in segs.into_iter().skip(1) {
                    pages.push(Vec::new());
                    if !seg.is_empty() {
                        pages.last_mut().expect("page exists").push(Inline::Link {
                            content: seg,
                            target: target.clone(),
                        });
                    }
                }
            }
            other => pages.last_mut().expect("page exists").push(other.clone()),
        }
    }
    pages
}

fn inlines_have_page_break(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::PageBreak => true,
        Inline::Link { content, .. } => inlines_have_page_break(content),
        _ => false,
    })
}

/// True when the table grid holds a page break anywhere (cells nest blocks).
fn table_has_page_break(table: &crate::model::Table) -> bool {
    table.grid.iter().flatten().any(|slot| match slot {
        CellSlot::Origin(cell) => blocks_have_page_break(&cell.blocks),
        CellSlot::Covered { .. } => false,
    })
}

fn blocks_have_page_break(blocks: &[Block]) -> bool {
    blocks.iter().any(|block| match block {
        Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
            inlines_have_page_break(inlines)
        }
        Block::List(list) => list.items.iter().any(|item| blocks_have_page_break(&item.blocks)),
        Block::Table(table) => table_has_page_break(table),
        Block::BlockQuote(inner) => blocks_have_page_break(inner),
        Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => false,
    })
}

/// True when nothing here would render: only blank paragraphs (the same
/// rule the Markdown renderer uses), so stray separators never make a page.
fn is_blank_page(blocks: &[Block]) -> bool {
    use crate::model::inlines_are_empty;
    blocks.iter().all(|block| match block {
        Block::Paragraph(inlines) => inlines_are_empty(inlines),
        _ => false,
    })
}

/// The notes a page needs: every note referenced from its blocks, plus the
/// transitive closure of notes those notes reference. A note two pages
/// reference appears on both — each page numbers its notes independently.
fn page_notes(blocks: &[Block], all: &[Note]) -> Vec<Note> {
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    collect_note_refs(blocks, &mut order, &mut seen);
    let mut i = 0;
    while i < order.len() {
        if let Some(note) = all.iter().find(|note| note.id == order[i]) {
            collect_note_refs(&note.blocks, &mut order, &mut seen);
        }
        i += 1;
    }
    all.iter().filter(|note| seen.contains(&note.id)).cloned().collect()
}

fn collect_note_refs(blocks: &[Block], order: &mut Vec<String>, seen: &mut HashSet<String>) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
                collect_inline_refs(inlines, order, seen);
            }
            Block::List(list) => {
                for item in &list.items {
                    collect_note_refs(&item.blocks, order, seen);
                }
            }
            Block::Table(table) => {
                for slot in table.grid.iter().flatten() {
                    if let CellSlot::Origin(cell) = slot {
                        collect_note_refs(&cell.blocks, order, seen);
                    }
                }
            }
            Block::BlockQuote(inner) => collect_note_refs(inner, order, seen),
            Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
        }
    }
}

fn collect_inline_refs(inlines: &[Inline], order: &mut Vec<String>, seen: &mut HashSet<String>) {
    for inline in inlines {
        match inline {
            Inline::NoteRef(id) => {
                if seen.insert(id.clone()) {
                    order.push(id.clone());
                }
            }
            Inline::Link { content, .. } => collect_inline_refs(content, order, seen),
            _ => {}
        }
    }
}

/// Keep only the assets the page's blocks and notes reference, remapping
/// [`AssetId`] indices (they are positions in `Document::assets`) so a page
/// stays a valid self-contained document.
fn retain_page_assets(page: &mut Document) {
    let mut used: Vec<AssetId> = Vec::new();
    let mut mark = |source: &ImageSource| {
        if let ImageSource::Asset(id) = source {
            if !used.contains(id) {
                used.push(*id);
            }
        }
    };
    collect_block_images(&page.blocks, &mut mark);
    for note in &page.notes {
        collect_block_images(&note.blocks, &mut mark);
    }
    // AssetId values are positions in Document::assets; parsers always
    // number assets densely, so equal lengths mean identical sets and the
    // remap below is a no-op.
    if used.len() == page.assets.len() {
        return;
    }
    let mut assets = Vec::with_capacity(used.len());
    for id in &used {
        let mut asset = page.assets[id.0].clone();
        asset.id = AssetId(assets.len());
        assets.push(asset);
    }
    let mut remap = |source: &mut ImageSource| {
        if let ImageSource::Asset(id) = source {
            let pos = used.iter().position(|used_id| used_id == id).expect("asset was marked");
            *id = AssetId(pos);
        }
    };
    walk_block_images(&mut page.blocks, &mut remap);
    for note in &mut page.notes {
        walk_block_images(&mut note.blocks, &mut remap);
    }
    page.assets = assets;
}

/// Visit every image source in the block tree, innermost links included.
fn collect_block_images(blocks: &[Block], f: &mut impl FnMut(&ImageSource)) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
                collect_inline_images(inlines, f);
            }
            Block::List(list) => {
                for item in &list.items {
                    collect_block_images(&item.blocks, f);
                }
            }
            Block::Table(table) => {
                for slot in table.grid.iter().flatten() {
                    if let CellSlot::Origin(cell) = slot {
                        collect_block_images(&cell.blocks, f);
                    }
                }
            }
            Block::BlockQuote(inner) => collect_block_images(inner, f),
            Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
        }
    }
}

fn collect_inline_images(inlines: &[Inline], f: &mut impl FnMut(&ImageSource)) {
    for inline in inlines {
        match inline {
            Inline::Image { source, .. } => f(source),
            Inline::Link { content, .. } => collect_inline_images(content, f),
            _ => {}
        }
    }
}

/// Mutable counterpart of [`collect_block_images`], for rewriting sources.
fn walk_block_images(blocks: &mut [Block], f: &mut impl FnMut(&mut ImageSource)) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => {
                walk_inline_images(inlines, f);
            }
            Block::List(list) => {
                for item in &mut list.items {
                    walk_block_images(&mut item.blocks, f);
                }
            }
            Block::Table(table) => {
                for slot in table.grid.iter_mut().flatten() {
                    if let CellSlot::Origin(cell) = slot {
                        walk_block_images(&mut cell.blocks, f);
                    }
                }
            }
            Block::BlockQuote(inner) => walk_block_images(inner, f),
            Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
        }
    }
}

fn walk_inline_images(inlines: &mut [Inline], f: &mut impl FnMut(&mut ImageSource)) {
    for inline in inlines {
        match inline {
            Inline::Image { source, .. } => f(source),
            Inline::Link { content, .. } => walk_inline_images(content, f),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Asset, ListItem, MarkerKind, NoteKind, Table, TableKind};
    use crate::render::markdown::document_to_markdown;

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![Inline::plain(text)])
    }

    fn page_markdown(doc: &Document) -> Vec<String> {
        paginate(doc).iter().map(document_to_markdown).collect()
    }

    #[test]
    fn no_breaks_is_a_single_page_matching_the_whole_document() {
        let doc = Document { blocks: vec![para("Alfa"), para("Beta")], ..Default::default() };
        let pages = paginate(&doc);
        assert_eq!(pages.len(), 1);
        assert_eq!(document_to_markdown(&pages[0]), document_to_markdown(&doc));
    }

    #[test]
    fn break_inside_a_paragraph_splits_it() {
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![
                Inline::plain("Alfa"),
                Inline::PageBreak,
                Inline::plain("Beta"),
            ])],
            ..Default::default()
        };
        assert_eq!(page_markdown(&doc), vec!["Alfa\n".to_string(), "Beta\n".to_string()]);
    }

    #[test]
    fn edge_and_repeated_breaks_never_make_blank_pages() {
        let doc = Document {
            blocks: vec![
                Block::Paragraph(vec![Inline::PageBreak]),
                Block::Paragraph(vec![
                    Inline::plain("Alfa"),
                    Inline::PageBreak,
                    Inline::PageBreak,
                    Inline::plain("Beta"),
                ]),
                Block::Paragraph(vec![Inline::PageBreak]),
            ],
            ..Default::default()
        };
        assert_eq!(page_markdown(&doc), vec!["Alfa\n".to_string(), "Beta\n".to_string()]);
    }

    #[test]
    fn heading_split_keeps_the_anchor_on_the_first_page() {
        let doc = Document {
            blocks: vec![Block::Heading {
                level: 1,
                anchor: Some("top".into()),
                content: vec![Inline::plain("Alfa"), Inline::PageBreak, Inline::plain("Beta")],
            }],
            ..Default::default()
        };
        let pages = paginate(&doc);
        assert_eq!(pages.len(), 2);
        match &pages[0].blocks[0] {
            Block::Heading { anchor, .. } => assert_eq!(anchor.as_deref(), Some("top")),
            other => panic!("unexpected block: {other:?}"),
        }
        match &pages[1].blocks[0] {
            Block::Heading { anchor, .. } => assert_eq!(anchor, &None),
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[test]
    fn link_spanning_a_break_survives_on_both_pages() {
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![Inline::Link {
                content: vec![Inline::plain("Alfa"), Inline::PageBreak, Inline::plain("Beta")],
                target: crate::model::LinkTarget::External("https://example.com".into()),
            }])],
            ..Default::default()
        };
        let md = page_markdown(&doc);
        assert_eq!(md, vec!["[Alfa](https://example.com)\n", "[Beta](https://example.com)\n"]);
    }

    #[test]
    fn list_item_split_continues_the_ordinals() {
        // A break after "two" (inside the second item) moves "three" to the
        // next page, where the list restarts at its original ordinal.
        let items = vec![
            ListItem { blocks: vec![para("one")], marker_label: None },
            ListItem { blocks: vec![Block::Paragraph(vec![
                Inline::plain("two"),
                Inline::PageBreak,
            ])], marker_label: None },
            ListItem { blocks: vec![para("three")], marker_label: None },
        ];
        let doc = Document {
            blocks: vec![Block::List(List {
                marker: MarkerKind::Decimal,
                start: 1,
                items,
            })],
            ..Default::default()
        };
        let md = page_markdown(&doc);
        assert_eq!(md, vec!["1. one\n2. two\n", "3. three\n"]);
    }

    #[test]
    fn table_with_an_inner_break_stays_whole() {
        let table = Table::from_rows(
            vec![vec![crate::model::Cell::from_inlines(vec![
                Inline::plain("cell"),
                Inline::PageBreak,
                Inline::plain("more"),
            ])]],
            0,
            TableKind::Data,
        );
        let doc = Document { blocks: vec![Block::Table(table)], ..Default::default() };
        let md = page_markdown(&doc);
        assert_eq!(md.len(), 1);
        assert!(md[0].contains("cell"));
        assert!(md[0].contains("more"));
    }

    #[test]
    fn notes_follow_the_page_that_references_them() {
        let body = |text: &str, id: &str| {
            Block::Paragraph(vec![Inline::plain(text), Inline::NoteRef(id.into())])
        };
        let doc = Document {
            blocks: vec![
                body("body one", "a"),
                Block::Paragraph(vec![Inline::PageBreak]),
                body("body two", "b"),
            ],
            notes: vec![
                Note { id: "a".into(), kind: NoteKind::Footnote, blocks: vec![para("note a")] },
                Note { id: "b".into(), kind: NoteKind::Footnote, blocks: vec![para("note b")] },
            ],
            assets: vec![],
        };
        let md = page_markdown(&doc);
        assert_eq!(md[0], "body one[^1]\n\n[^1]: note a\n");
        assert_eq!(md[1], "body two[^1]\n\n[^1]: note b\n");
    }

    #[test]
    fn nested_note_references_are_pulled_into_the_page() {
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![
                Inline::plain("body"),
                Inline::NoteRef("a".into()),
            ])],
            notes: vec![
                Note {
                    id: "a".into(),
                    kind: NoteKind::Footnote,
                    blocks: vec![Block::Paragraph(vec![
                        Inline::plain("see b"),
                        Inline::NoteRef("b".into()),
                    ])],
                },
                Note { id: "b".into(), kind: NoteKind::Footnote, blocks: vec![para("note b")] },
            ],
            assets: vec![],
        };
        let md = page_markdown(&doc);
        assert_eq!(md.len(), 1);
        assert!(md[0].contains("[^1]: see b[^2]"));
        assert!(md[0].contains("[^2]: note b"));
    }

    #[test]
    fn one_note_referenced_from_two_pages_appears_on_both() {
        let body = |text: &str| {
            Block::Paragraph(vec![Inline::plain(text), Inline::NoteRef("shared".into())])
        };
        let doc = Document {
            blocks: vec![
                body("page one"),
                Block::Paragraph(vec![Inline::PageBreak]),
                body("page two"),
            ],
            notes: vec![Note {
                id: "shared".into(),
                kind: NoteKind::Endnote,
                blocks: vec![para("shared note")],
            }],
            assets: vec![],
        };
        let pages = paginate(&doc);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].notes.len(), 1);
        assert_eq!(pages[1].notes.len(), 1);
        assert_eq!(pages[0].notes[0].kind, NoteKind::Endnote);
    }

    #[test]
    fn assets_are_filtered_and_reindexed_per_page() {
        let img = |n: usize| Inline::Image {
            alt: format!("img{n}"),
            source: ImageSource::Asset(AssetId(n)),
        };
        let asset = |n: usize, part: &str| Asset {
            id: AssetId(n),
            media_type: "image/png".into(),
            origin_part: part.into(),
            bytes: vec![n as u8],
        };
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![img(0), Inline::PageBreak, img(1)])],
            assets: vec![asset(0, "a.png"), asset(1, "b.png")],
            ..Default::default()
        };
        let pages = paginate(&doc);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].assets.len(), 1);
        assert_eq!(pages[0].assets[0].origin_part, "a.png");
        assert_eq!(pages[0].assets[0].id, AssetId(0));
        assert_eq!(pages[1].assets.len(), 1);
        assert_eq!(pages[1].assets[0].origin_part, "b.png");
        let Block::Paragraph(inlines) = &pages[1].blocks[0] else { panic!() };
        assert!(
            matches!(&inlines[0], Inline::Image { source: ImageSource::Asset(id), .. } if *id == AssetId(0)),
            "the reference must be rewritten to the page-local index: {:?}",
            inlines[0]
        );
    }

    #[test]
    fn a_document_of_only_breaks_still_returns_one_page() {
        let doc = Document {
            blocks: vec![Block::Paragraph(vec![Inline::PageBreak])],
            ..Default::default()
        };
        let pages = paginate(&doc);
        assert_eq!(pages.len(), 1);
        assert_eq!(document_to_markdown(&pages[0]), "");
    }
// --- Review regression tests: anchors and links around leading breaks. ---

#[test]
fn heading_with_a_leading_break_keeps_its_anchor() {
    let doc = Document {
        blocks: vec![Block::Heading {
            level: 1,
            anchor: Some("top".into()),
            content: vec![Inline::PageBreak, Inline::plain("Beta")],
        }],
        ..Default::default()
    };
    let pages = paginate(&doc);
    assert_eq!(pages.len(), 1, "the empty first segment drops with the blank page");
    match &pages[0].blocks[0] {
        Block::Heading { anchor, .. } => assert_eq!(anchor.as_deref(), Some("top")),
        other => panic!("unexpected block: {other:?}"),
    }
}

#[test]
fn link_with_a_leading_break_renders_no_empty_label() {
    let doc = Document {
        blocks: vec![Block::Paragraph(vec![Inline::Link {
            content: vec![Inline::PageBreak, Inline::plain("Beta")],
            target: crate::model::LinkTarget::External("https://example.com".into()),
        }])],
        ..Default::default()
    };
    let md = page_markdown(&doc);
    // The break moved the label to the following page; the link still wraps
    // it, and no bare `[url](url)` noise appears.
    assert_eq!(md, vec!["[Beta](https://example.com)\n".to_string()]);
}
}
