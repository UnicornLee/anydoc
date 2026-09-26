//! Convert a document to one Markdown file per page.
//!
//! Usage: cargo run --example doc_to_pages -- <input-file> [output-dir]

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next().map(PathBuf::from) else {
        eprintln!("usage: doc_to_pages <input-file> [output-dir]");
        std::process::exit(2);
    };
    let out_dir = args.next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("pages"));

    let pages = match anydoc::to_pages(&input) {
        Ok(pages) => pages,
        Err(err) => {
            eprintln!("{}: {err}", input.display());
            std::process::exit(1);
        }
    };
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        eprintln!("{}: {err}", out_dir.display());
        std::process::exit(1);
    }
    println!("{}: {} page(s)", input.display(), pages.len());
    for (idx, page) in pages.iter().enumerate() {
        let out = out_dir.join(format!("page-{:02}.md", idx + 1));
        if let Err(err) = std::fs::write(&out, page) {
            eprintln!("{}: {err}", out.display());
            std::process::exit(1);
        }
        println!("  {} ({} bytes)", out.display(), page.len());
    }
}
