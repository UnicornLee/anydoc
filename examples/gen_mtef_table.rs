//! Generate `src/shared/math/mtef_chars.rs` from the reference `chars.py` dict.
//!
//! `cargo run --example gen_mtef_table`

use std::fs;

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn main() -> std::io::Result<()> {
    let src = fs::read_to_string("mtef_ref/chars.py").expect("read chars.py");
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for line in src.lines() {
        let line = line.trim();
        if !line.starts_with('"') {
            continue;
        }
        // key = first quoted string
        let Some(a) = line.find('"') else { continue };
        let Some(b) = line[a + 1..].find('"') else { continue };
        let key = &line[a + 1..a + 1 + b];
        if !key.starts_with("char/0x") {
            continue; // skip SpecialChar dict entries
        }
        eprintln!("DBG key={:?} line={:?}", key, line);
        // value = string after the first `": "`
        let rest = &line[a + 1 + b..];
        let Some(colon) = rest.find("\": \"") else { continue };
        let vstart = a + 1 + b + 1 + colon + 3;
        let vtail = &line[vstart..];
        let Some(c) = vtail.find('"') else { continue };
        let raw = &vtail[..c];
        eprintln!("DBG insert key={:?} raw={:?}", key, raw);
        map.insert(key.to_string(), unescape(raw));
    }

    let mut out = String::from(
        "// AUTO-GENERATED from mtef_ref/chars.py by examples/gen_mtef_table.rs. Do not edit.\n\
         /// MTCode -> LaTeX lookup table (see MathType MTEF spec). Binary searched by key.\n\
         pub static MTEF_CHARS: &[(&str, &str)] = &[\n",
    );
    for (k, v) in &map {
        out.push_str(&format!("    ({:?}, {:?}),\n", k, v));
    }
    out.push_str("];\n");

    fs::write("src/shared/math/mtef_chars.rs", out)?;
    eprintln!("wrote {} entries", map.len());
    Ok(())
}
