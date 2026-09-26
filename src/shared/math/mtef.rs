//! MTEF (MathType Equation Format) decoder.
//!
//! MathType / Microsoft Equation Editor 3.0 OLE objects in a `.docx` store the
//! formula as an `Equation Native` stream inside an OLE2 compound file. That
//! stream is MTEF, a binary record format. This module turns it into LaTeX,
//! so anydoc can render those formulas instead of emitting
//! `Embedded object: Equation.3` / `Equation.DSMT4`.
//!
//! The record layout follows the MathType MTEF v3/v5 specification and the
//! open-source `mathtypejx` decoder.

use std::io::{Cursor, Read};

#[path = "mtef_chars.rs"]
mod chars;

// ---------------------------------------------------------------------------
// Low level reader
// ---------------------------------------------------------------------------

struct R<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        R { b, pos: 0 }
    }
    fn u8(&mut self) -> Option<u8> {
        if self.pos >= self.b.len() {
            return None;
        }
        let v = self.b[self.pos];
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.u8()?, self.u8()?]))
    }
    fn eof(&self) -> bool {
        self.pos >= self.b.len()
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const OLE_CB_HDR: usize = 28;

mod tag {
    pub const ROOT: u8 = 0xff;
    pub const END: u8 = 0;
    pub const LINE: u8 = 1;
    pub const CHAR: u8 = 2;
    pub const TMPL: u8 = 3;
    pub const PILE: u8 = 4;
    pub const MATRIX: u8 = 5;
    pub const EMBELL: u8 = 6;
    pub const RULER: u8 = 7;
    pub const FONT_STYLE_DEF: u8 = 8;
    pub const SIZE: u8 = 9;
    pub const FULL: u8 = 10;
    pub const SUB: u8 = 11;
    pub const SUB2: u8 = 12;
    pub const SYM: u8 = 13;
    pub const SUBSYM: u8 = 14;
    pub const COLOR: u8 = 15;
    pub const COLOR_DEF: u8 = 16;
    pub const FONT_DEF: u8 = 17;
    pub const EQN_PREFS: u8 = 18;
    pub const ENCODING_DEF: u8 = 19;
}

mod opt {
    pub const NUDGE: u8 = 0x08;
    pub const CHAR_EMBELL: u8 = 0x01;
    pub const CHAR_ENC_CHAR8: u8 = 0x04;
    pub const CHAR_ENC_CHAR16: u8 = 0x10;
    pub const CHAR_ENC_NO_MTCODE: u8 = 0x20;
    pub const LINE_NULL: u8 = 0x01;
    pub const LINE_RULER: u8 = 0x02;
    pub const LINE_LSPACE: u8 = 0x04;
    pub const COLOR_CMYK: u8 = 0x01;
    pub const COLOR_NAME: u8 = 0x04;
}

mod sel {
    pub const ANGLE: u8 = 0;
    pub const PAREN: u8 = 1;
    pub const BRACE: u8 = 2;
    pub const BRACK: u8 = 3;
    pub const BAR: u8 = 4;
    pub const DBAR: u8 = 5;
    pub const FLOOR: u8 = 6;
    pub const CEILING: u8 = 7;
    pub const OBAR: u8 = 8;
    pub const INTERVAL: u8 = 9;
    pub const ROOT: u8 = 10;
    pub const FRACT: u8 = 11;
    pub const UBAR: u8 = 12;
    pub const OBAR_T: u8 = 13;
    pub const ARROW: u8 = 14;
    pub const INTEG: u8 = 15;
    pub const SUM: u8 = 16;
    pub const PROD: u8 = 17;
    pub const COPROD: u8 = 18;
    pub const UNION: u8 = 19;
    pub const INTER: u8 = 20;
    pub const INTOP: u8 = 21;
    pub const SUMOP: u8 = 22;
    pub const LIM: u8 = 23;
    pub const HBRACE: u8 = 24;
    pub const HBRACK: u8 = 25;
    pub const LDIV: u8 = 26;
    pub const SUB: u8 = 27;
    pub const SUP: u8 = 28;
    pub const SUBSUP: u8 = 29;
    pub const DIRAC: u8 = 30;
    pub const VEC: u8 = 31;
    pub const TILDE: u8 = 32;
    pub const HAT: u8 = 33;
    pub const ARC: u8 = 34;
    pub const JSTATUS: u8 = 35;
    pub const STRIKE: u8 = 36;
    pub const BOX: u8 = 37;
}

mod emb {
    pub const D1DOT: u8 = 2;
    pub const D2DOT: u8 = 3;
    pub const D3DOT: u8 = 4;
    pub const D1PRIME: u8 = 5;
    pub const D2PRIME: u8 = 6;
    pub const DBAR: u8 = 7;
    pub const TILDE: u8 = 8;
    pub const HAT: u8 = 9;
    pub const NOT: u8 = 10;
    pub const RARROW: u8 = 11;
    pub const LARROW: u8 = 12;
    pub const BARROW: u8 = 13;
    pub const R1ARROW: u8 = 14;
    pub const L1ARROW: u8 = 15;
    pub const MBAR: u8 = 16;
    pub const OBAR: u8 = 17;
    pub const D3PRIME: u8 = 18;
    pub const FROWN: u8 = 19;
    pub const SMILE: u8 = 20;
    pub const X_BARS: u8 = 21;
    pub const UP_BAR: u8 = 22;
    pub const DOWN_BAR: u8 = 23;
    pub const D4DOT: u8 = 24;
    pub const U_1DOT: u8 = 25;
    pub const U_2DOT: u8 = 26;
    pub const U_3DOT: u8 = 27;
    pub const U_4DOT: u8 = 28;
    pub const U_BAR: u8 = 29;
    pub const U_TILDE: u8 = 30;
    pub const U_FROWN: u8 = 31;
    pub const U_SMILE: u8 = 32;
    pub const U_RARROW: u8 = 33;
    pub const U_LARROW: u8 = 34;
    pub const U_BARROW: u8 = 35;
    pub const U_R1ARROW: u8 = 36;
    pub const U_L1ARROW: u8 = 37;
}

// ---------------------------------------------------------------------------
// Node tree (arena + indices, to stay sound across Vec reallocations)
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Node {
    tag: u8,
    null: bool,
    mtcode: u16,
    typeface: u8,
    selector: u8,
    variation: u16,
    embell: u8,
    embellished: bool,
    children: Vec<usize>,
}

/// Flat record produced by the byte parser, before the tree is assembled.
#[derive(Default, Clone)]
struct Rec {
    tag: u8,
    null: bool,
    mtcode: u16,
    typeface: u8,
    selector: u8,
    variation: u16,
    embell: u8,
    /// v3 CHAR records carry their superscript/subscript as an *embellishment
    /// list* (a nested record sequence terminated by END) instead of a TMPL
    /// wrapping the character. We push such a CHAR onto the arena stack so the
    /// following records nest under it, then render the scripts in `latex`.
    embellished: bool,
}

// ---------------------------------------------------------------------------
// MTEF v3 → v5 template selector translation
// ---------------------------------------------------------------------------
//
// MTEF v3 (Equation Editor 3.0) and v5 (MathType) number their template
// selectors differently for the *same* construct. v3 superscript = selector 15,
// while v5 superscript = 28 (and v5 15 is INTEG). Decoding v3 without
// remapping mis-renders superscripts as integrals, fractions as arrows, etc.
// The mapping below is taken verbatim from `mathtypejx/mtef/records3.py`
// (`SELECTORS` / `VARIATIONS` / `SCRIPT_SELECTORS`).
//
// Returns the v5 `(selector, variation)` pair that `tmpl()` expects.
fn v3_tmpl_selector(sel: u8, variation: u16) -> (u8, u16) {
    match sel {
        // Script templates: variation picks sub / sup / subsup.
        15 | 44 => match variation & 0x0003 {
            1 => (sel::SUB, 0),
            2 => (sel::SUBSUP, 0),
            _ => (sel::SUP, 0), // 0 and 3 -> superscript
        },
        0 => (sel::ANGLE, 0),
        1 => (sel::PAREN, 0),
        2 => (sel::BRACE, 0),
        3 => (sel::BRACK, 0),
        4 => (sel::BAR, 0),
        5 => (sel::DBAR, 0),
        6 => (sel::FLOOR, 0),
        7 => (sel::CEILING, 0),
        8 | 9 | 10 | 11 | 12 => (sel::INTERVAL, 0),
        13 => (sel::ROOT, 0),
        14 | 41 => (sel::FRACT, 0),
        16 => (sel::UBAR, 0),
        17 => (sel::OBAR_T, 0),
        18 | 19 | 20 => (sel::ARROW, 0),
        // Single / double / triple integrals are distinct v3 selectors; v5
        // encodes the multiplicity in the variation's low nibble.
        21 => (sel::INTEG, 0),
        22 => (sel::INTEG, 1),
        23 => (sel::INTEG, 2),
        24 | 25 | 26 => (sel::INTEG, 3),
        27 | 28 => (sel::HBRACE, 0),
        29 | 30 => (sel::SUM, 0),
        31 | 32 => (sel::PROD, 0),
        33 | 34 => (sel::COPROD, 0),
        35 | 36 => (sel::UNION, 0),
        37 | 38 => (sel::INTER, 0),
        39 => (sel::LIM, 0),
        40 => (sel::LDIV, 0),
        42 => (sel::INTOP, 0),
        43 => (sel::SUMOP, 0),
        45 => (sel::DIRAC, 0),
        46 | 47 => (sel::VEC, 0),
        48 => (sel::BOX, 0),
        _ => (sel::ANGLE, 0), // unknown -> safe default, degrades not crashes
    }
}

// ---------------------------------------------------------------------------
// Decode entry point
// ---------------------------------------------------------------------------

/// Decode an `Equation Native` MTEF stream out of an OLE2 object (the bytes of
/// `word/embeddings/oleObjectN.bin`). Returns LaTeX without `$` delimiters, or
/// `None` if the data is not a recognizable MTEF formula.
pub fn decode_ole_bytes(ole_bytes: &[u8]) -> Option<String> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(ole_bytes)).ok()?;
    let name = cf
        .read_root_storage()
        .find(|e| e.name().contains("Equation Native"))
        .map(|e| e.name().to_string())?;
    let mut stream = cf.open_stream(&name).ok()?;
    let mut data = Vec::new();
    stream.read_to_end(&mut data).ok()?;
    decode_stream(&data)
}

/// Decode a raw `Equation Native` stream (already extracted from the OLE2 CFB).
pub fn decode_stream(data: &[u8]) -> Option<String> {
    if data.len() < OLE_CB_HDR {
        return None;
    }
    let cb_hdr = u16::from_le_bytes([data[0], data[1]]) as usize;
    if cb_hdr != OLE_CB_HDR {
        return None;
    }
    let cb_size = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let start = OLE_CB_HDR;
    let end = (start + cb_size).min(data.len());
    let body = &data[start..end];
    if body.is_empty() {
        return None;
    }

    let mut r = R::new(body);
    // MTEF version is the first header byte. Equation.3 (the OLE object MathType
    // branded "Equation.3", i.e. Microsoft Equation Editor) writes MTEF v3, where the
    // record tag byte packs the type in its LOW nibble and the options/flags in the
    // HIGH nibble (no separate options byte follows). MathType itself writes v5, where
    // the tag byte is the bare type and a distinct options byte follows. LibreOffice's
    // importer reads `nRecord = nTag & 0x0f` for exactly this reason.
    let version = r.b[r.pos];
    let is_v3 = version <= 3;
    // MTEF header: version, platform, product, version, subversion.
    let _ = r.u8()?;
    let _ = r.u8()?;
    let _ = r.u8()?;
    let _ = r.u8()?;
    let _ = r.u8()?;
    // Application name (null terminated) plus one "inline" flag byte exist
    // only in MathType (v5) streams: `05 .. "DSMT5" 00 01 <records>`.
    // Equation Editor (v3) streams jump straight from the 5-byte header to the
    // records: `03 .. 01 0a 01 ..`. Reading a name there eats real equation
    // content (the leading `(` in `(x..)`, or the whole `k` in a one-letter
    // formula), so v3 must not consume one. Verified against all corpus v3
    // objects, where dropping this fixes empty/truncated output.
    if !is_v3 {
        while !r.eof() && r.b[r.pos] != 0 {
            r.pos += 1;
        }
        if !r.eof() {
            r.pos += 1;
        }
        let _ = r.u8()?;
    }

    let mut nodes: Vec<Rec> = Vec::new();
    while !r.eof() {
        // v3: type in low nibble, opts/flags in high nibble, no separate opts byte.
        // v5: bare type byte, then a separate options byte.
        let (t, opts) = if is_v3 {
            let byte = r.u8()?;
            (byte & 0x0F, byte >> 4)
        } else {
            let t = r.u8()?;
            if t >= 100 {
                // FUTURE record: 1 length byte + payload. A truncated trailing
                // FUTURE has no length byte; stop and keep what we have.
                let Some(skip) = r.u8() else { break };
                r.pos += skip as usize;
                continue;
            }
            // END is unique: it is a bare tag byte with NO options byte and no
            // data (MTEF5 spec). Reading an options byte after it desyncs every
            // following record by one.
            if t == tag::END {
                nodes.push(Rec { tag: t, ..Default::default() });
                continue;
            }
            // A lone trailing byte (e.g. stray RULER/padding after the closing
            // END) has no options byte; stop and keep the equation parsed so far
            // instead of discarding the whole decode.
            let Some(opts) = r.u8() else { break };
            (t, opts)
        };
        if std::env::var_os("MTEF_TRACE").is_some() {
            eprintln!("  @{} tag={} opts={:#x} (v3={})", r.pos.saturating_sub(1), t, opts, is_v3);
        }
        match t {
            tag::END => nodes.push(Rec { tag: t, ..Default::default() }),
            tag::LINE => {
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                if opts & opt::LINE_LSPACE != 0 {
                    let _ = r.u8();
                }
                if opts & opt::LINE_RULER != 0 {
                    let n = r.u8()?;
                    for _ in 0..n {
                        let _ = r.u8();
                        let _ = r.u16();
                    }
                }
                let null = opts & opt::LINE_NULL != 0;
                nodes.push(Rec { tag: t, null, ..Default::default() });
            }
            tag::CHAR => {
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                // v3 CHAR_EMBELL introduces a nested embellishment record list
                // (scripts/accents) that the main loop will consume as nested
                // children of this CHAR. v5 also uses CHAR_EMBELL, but its
                // embellishment encoding differs and is left to push the char
                // flat to avoid regressing the v5 path.
                let embellished = is_v3 && (opts & opt::CHAR_EMBELL != 0);
                if is_v3 {
                    // v3: typeface byte FIRST, then 16-bit mtcode (LE), no
                    // CHAR_ENC_* bytes. CHAR_EMBELL means EMBELL records follow,
                    // not inline bytes, so nothing extra is consumed here.
                    let typeface = r.u8()?;
                    let mtcode = r.u16()?;
                    nodes.push(Rec { tag: t, mtcode, typeface, embellished, ..Default::default() });
                } else {
                    let typeface = r.u8()?;
                    let mut mtcode = 0u16;
                    if opts & opt::CHAR_ENC_NO_MTCODE == 0 {
                        mtcode = r.u16()?;
                    }
                    if opts & opt::CHAR_ENC_CHAR8 != 0 {
                        let _ = r.u8();
                    }
                    if opts & opt::CHAR_ENC_CHAR16 != 0 {
                        let _ = r.u16();
                    }
                    nodes.push(Rec { tag: t, mtcode, typeface, ..Default::default() });
                }
            }
            tag::TMPL => {
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let mut selector = r.u8()?;
                let b1 = r.u8()?;
                let mut variation = if b1 & 0x80 != 0 {
                    let b2 = r.u8()?;
                    ((b1 & 0x7f) as u16) | ((b2 as u16) << 8)
                } else {
                    b1 as u16
                };
                let _ = r.u8(); // tmpl.options
                // v3 numbers template selectors differently from v5; remap so
                // the shared `tmpl()` emitter produces the right LaTeX.
                if is_v3 {
                    let mapped = v3_tmpl_selector(selector, variation);
                    selector = mapped.0;
                    variation = mapped.1;
                }
                nodes.push(Rec { tag: t, selector, variation, ..Default::default() });
            }
            tag::PILE => {
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let _ = r.u8();
                let _ = r.u8();
                nodes.push(Rec { tag: t, ..Default::default() });
            }
            tag::MATRIX => {
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let _ = r.u8();
                let _ = r.u8();
                let _ = r.u8();
                let _ = r.u8();
                let _ = r.u8();
                nodes.push(Rec { tag: t, ..Default::default() });
            }
            tag::EMBELL => {
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let embell = r.u8()?;
                nodes.push(Rec { tag: t, embell, ..Default::default() });
            }
            tag::FONT_STYLE_DEF => {
                let _ = r.u8();
                skip_cstr(&mut r);
            }
            tag::SIZE => {
                let _ = r.u8();
                let _ = r.u8();
            }
            tag::SUB | tag::SUB2 | tag::SYM | tag::SUBSYM => {
                nodes.push(Rec { tag: t, ..Default::default() });
            }
            tag::FONT_DEF => {
                let _ = r.u8();
                skip_cstr(&mut r);
            }
            tag::COLOR => {
                // v5 carries a separate options byte before the color index.
                if !is_v3 {
                    let _ = r.u8();
                }
                let _ = r.u8(); // color index
            }
            tag::COLOR_DEF => {
                // v5 carries a separate options byte; v3 keeps opts in the tag's high nibble.
                let opts = if !is_v3 { r.u8()? } else { opts };
                if opts & opt::COLOR_CMYK != 0 {
                    for _ in 0..4 {
                        let _ = r.u16();
                    }
                } else {
                    for _ in 0..3 {
                        let _ = r.u16();
                    }
                }
                if opts & opt::COLOR_NAME != 0 {
                    skip_cstr(&mut r);
                }
            }
            tag::FULL => nodes.push(Rec { tag: t, ..Default::default() }),
            tag::EQN_PREFS => {
                // Only reached for v5 (in v3 a 0x12 byte is CHAR). The record's
                // options byte was already consumed by the tag loop above, so
                // the sizes count is next. Layout, verified byte-for-byte
                // against MathType "DSMT5" streams:
                //   sizes  = count byte + that many nibble-packed dimensions
                //   spaces = count byte + that many nibble-packed dimensions
                //   styles = count byte + that many (font_def, char_style) pairs
                let n = r.u8()?;
                dimension_arrays(&mut r, n)?;
                let n = r.u8()?;
                dimension_arrays(&mut r, n)?;
                // Styles: each entry is one byte; a second byte follows only when
                // the first is non-zero (a zero marks "no style"). Reading two
                // bytes unconditionally desyncs the stream by one per zero style,
                // which broke every MathType (v5) object carrying EQN_PREFS.
                let n = r.u8()?;
                for _ in 0..n {
                    let c = r.u8()?;
                    if c != 0 {
                        let _ = r.u8()?;
                    }
                }
            }
            tag::ENCODING_DEF => {
                skip_cstr(&mut r);
            }
            tag::RULER => {
                // Standalone ruler record: nudge, then stop count and the
                // (type, offset) pairs. Rarely carries equation content.
                if opts & opt::NUDGE != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let n = r.u8()?;
                for _ in 0..n {
                    let _ = r.u8();
                    let _ = r.u16();
                }
            }
            // Unknown or trailing padding byte: stop parsing and keep whatever
            // equation content was already collected. Returning None here used
            // to discard complete formulas (e.g. `x=0`, `b`) whose stream ends
            // with a stray RULER/padding byte after the closing END. The empty
            // output check after the loop still rejects content-free streams.
            _ => break,
        }
    }

    let arena = build_ast(nodes);
    let mut out = String::new();
    for &c in &arena[0].children {
        latex(&arena, c, &mut out);
    }
    if out.trim().is_empty() { None } else { Some(out) }
}

fn skip_cstr(r: &mut R<'_>) {
    while !r.eof() && r.b[r.pos] != 0 {
        r.pos += 1;
    }
    if !r.eof() {
        r.pos += 1;
    }
}

/// Consumes one packed dimension array. Returns `None` if the stream ends
/// first. `count` only advances on the 0x0f terminator, so a stream without
/// one would otherwise loop forever - and reading past the end used to keep
/// yielding 0 while pushing into `_array` until the process exhausted memory.
/// The values themselves are irrelevant (anydoc only needs to skip them), so
/// this just walks the packed nibbles until `size` dimensions are terminated.
///
/// Each dimension is a unit nibble followed by digit nibbles, ended by 0x0f.
/// The array is packed two nibbles per byte and padded to a byte boundary, so
/// the half-eaten byte at the end must be dropped: otherwise the padding nibble
/// is read as the next unit and every following byte is off by one.
fn dimension_arrays(r: &mut R<'_>, size: u8) -> Option<()> {
    let mut count = 0u8;
    let mut expect_unit = true;
    let mut pending_lo: Option<u8> = None;
    while count < size {
        let nib = match pending_lo.take() {
            Some(n) => n,
            None => {
                let ch = r.u8()?;
                pending_lo = Some(ch & 0x0f);
                (ch & 0xf0) >> 4
            }
        };
        if expect_unit {
            // Unit nibble (in/cm/pt/pc/%) - discarded, only its position matters.
            expect_unit = false;
        } else if nib == 0x0f {
            expect_unit = true;
            count += 1;
        }
    }
    // Align back to the byte boundary for the next record/count byte.
    pending_lo = None;
    Some(())
}

// Stack based, faithful to the reference decoder. The tree is materialised in
// an arena (Vec<Node>) with child references as indices, so it stays sound even
// though the `children` Vecs reallocate as we push.
fn build_ast(nodes: Vec<Rec>) -> Vec<Node> {
    let mut arena: Vec<Node> = Vec::new();
    arena.push(Node { tag: 0xff, ..Default::default() }); // root at index 0
    let mut stack: Vec<usize> = vec![0];
    for rec in nodes {
        // A malformed stream can carry more END records than openers, and the
        // root is popped with the rest of the stack if nothing guards it. Fall
        // back to the root instead of unwinding: the equation degrades, but a
        // single bad record no longer takes down the whole document.
        let parent = stack.last().copied().unwrap_or(0);
        match rec.tag {
            tag::END => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            tag::LINE => {
                let idx = arena.len();
                arena.push(Node { tag: rec.tag, null: rec.null, ..Default::default() });
                arena[parent].children.push(idx);
                if !rec.null {
                    stack.push(idx);
                }
            }
            tag::CHAR => {
                let idx = arena.len();
                arena.push(Node {
                    tag: rec.tag,
                    mtcode: rec.mtcode,
                    typeface: rec.typeface,
                    ..Default::default()
                });
                arena[parent].children.push(idx);
            }
            tag::TMPL | tag::PILE | tag::MATRIX => {
                let idx = arena.len();
                arena.push(Node {
                    tag: rec.tag,
                    selector: rec.selector,
                    variation: rec.variation,
                    ..Default::default()
                });
                arena[parent].children.push(idx);
                stack.push(idx);
            }
            tag::EMBELL => {
                let idx = arena.len();
                arena.push(Node { tag: rec.tag, embell: rec.embell, ..Default::default() });
                arena[parent].children.push(idx);
                if (rec.embell == emb::D1DOT || rec.embell == emb::HAT || rec.embell == emb::OBAR)
                    && arena[parent].children.len() >= 2
                {
                    let n = arena[parent].children.len();
                    arena[parent].children.swap(n - 1, n - 2);
                }
                stack.push(idx);
            }
            _ => {
                let idx = arena.len();
                arena.push(Node { tag: rec.tag, ..Default::default() });
                arena[parent].children.push(idx);
            }
        }
    }
    arena
}

// ---------------------------------------------------------------------------
// LaTeX emission
// ---------------------------------------------------------------------------

fn latex(arena: &[Node], idx: usize, out: &mut String) {
    let node = &arena[idx];
    match node.tag {
        tag::ROOT => {
            for &c in &node.children {
                latex(arena, c, out);
            }
        }
        tag::LINE | tag::PILE => {
            for &c in &node.children {
                latex(arena, c, out);
            }
        }
        tag::CHAR => {
            out.push_str(&char_latex(node.mtcode, node.typeface));
        }
        tag::MATRIX => {
            let mut body = String::new();
            for &c in &node.children {
                latex(arena, c, &mut body);
            }
            out.push_str("\\begin{matrix} ");
            out.push_str(&body);
            out.push_str(" \\end{matrix}");
        }
        tag::TMPL => tmpl(arena, idx, out),
        tag::EMBELL => embell(arena, idx, out),
        _ => {
            for &c in &node.children {
                latex(arena, c, out);
            }
        }
    }
}

fn child_latex(arena: &[Node], idx: usize, child: usize) -> String {
    if let Some(&c) = arena[idx].children.get(child) {
        let mut s = String::new();
        latex(arena, c, &mut s);
        s
    } else {
        String::new()
    }
}

/// Render a single arena node (by index) to its LaTeX, used for embellishment
/// script lines and other inlined sub-expressions.
fn render_node(arena: &[Node], idx: usize) -> String {
    let mut s = String::new();
    latex(arena, idx, &mut s);
    s
}

/// Whether a record tag is a "size" placeholder (MTEF SIZE/FULL/SUB/SUB2/SYM/
/// SUBSYM). v3 embellishment sup/sub templates carry one of these as their first
/// child, with the real script in a later non-null LINE. SUB (subscript-size) is
/// a legitimate size marker too; omitting it dropped every exponent whose
/// embellishment used it (e.g. the `x^2` in `\frac{x^2}{1+x^2}`).
fn is_size(t: u8) -> bool {
    t == tag::SIZE
        || t == tag::FULL
        || t == tag::SUB
        || t == tag::SUB2
        || t == tag::SYM
        || t == tag::SUBSYM
}

/// The real operand LINEs of a template, skipping v3 size markers (SIZE/FULL/
/// SUB/…) and transparent wrappers that sit between slots. MTEF template slots
/// are always LINEs, so filtering to them is spec-safe for both v3 and v5.
fn slot_lines(arena: &[Node], idx: usize) -> Vec<usize> {
    arena[idx].children.iter().copied().filter(|&c| arena[c].tag == tag::LINE).collect()
}

/// Extract the superscript (or subscript) text of a sup/sub template.
///
/// * Normal template: children are `[base, script]` -> `script = child[1]`.
/// * v3 embellishment template: children are `[size, base(null line), script
///   line]` -> the script is the first non-null LINE after the size node.
fn script_operand(arena: &[Node], idx: usize) -> String {
    let children = &arena[idx].children;
    if children.first().map_or(false, |&c| is_size(arena[c].tag)) {
        for &c in &children[1..] {
            if arena[c].tag == tag::LINE && !arena[c].null {
                return render_node(arena, c);
            }
        }
        String::new()
    } else {
        child_latex(arena, idx, 1)
    }
}

/// Extract (sub, sup) for a subsup template, handling both layouts.
fn subsup_operands(arena: &[Node], idx: usize) -> (String, String) {
    let children = &arena[idx].children;
    if children.first().map_or(false, |&c| is_size(arena[c].tag)) {
        let lines: Vec<String> = children[1..]
            .iter()
            .filter(|&&c| arena[c].tag == tag::LINE && !arena[c].null)
            .map(|&c| render_node(arena, c))
            .collect();
        let sub = lines.first().cloned().unwrap_or_default();
        let sup = lines.get(1).cloned().unwrap_or_default();
        (sub, sup)
    } else {
        (child_latex(arena, idx, 1), child_latex(arena, idx, 2))
    }
}

fn char_latex(mtcode: u16, typeface: u8) -> String {
    let tf = typeface as i32 - 128;
    let hex_extend = if tf == 11 || tf == 24 { "/mathmode" } else { "" };
    let typeface_fmt = tf == 1; // fnTEXT

    let hex_code = format!("{:04x}", mtcode);
    let hex_key = format!("char/0x{}{}", hex_code, hex_extend);

    let mut ch = char::from_u32(mtcode as u32).unwrap_or('?').to_string();
    if let Some(s) = chars::MTEF_CHARS
        .binary_search_by(|(k, _)| k.as_bytes().cmp(hex_key.as_bytes()))
        .ok()
        .map(|i| chars::MTEF_CHARS[i].1)
    {
        ch = s.to_string();
    } else if let Some(s) = special_char(&ch) {
        ch = s.to_string();
    }

    if typeface_fmt { format!("{{ \\rm{{ {ch} }} }}") } else { ch }
}

fn special_char(c: &str) -> Option<&'static str> {
    match c {
        "{" => Some("\\{"),
        "}" => Some("\\}"),
        "\\" => Some("\\textbackslash "),
        "^" => Some("\\^{}"),
        "_" => Some("\\_"),
        "~" => Some("\\textasciitilde "),
        "&" => Some("\\&"),
        "%" => Some("\\%"),
        "#" => Some("\\#"),
        "$" => Some("\\$"),
        "|" => Some("\\|"),
        _ => None,
    }
}

fn tmpl(arena: &[Node], idx: usize, out: &mut String) {
    let s = arena[idx].selector;
    let v = arena[idx].variation;
    match s {
        sel::PAREN
        | sel::ANGLE
        | sel::BRACK
        | sel::BAR
        | sel::DBAR
        | sel::FLOOR
        | sel::CEILING
        | sel::OBAR
        | sel::INTERVAL => {
            let main = child_latex(arena, idx, 0);
            let left = child_latex(arena, idx, 1);
            let right = child_latex(arena, idx, 2);
            let left_str = if left.is_empty() { String::new() } else { format!("\\left {}", left) };
            let right_str =
                if right.is_empty() { ".".to_string() } else { format!(" \\right {}", right) };
            out.push_str(&format!("{left_str} {{ {main} }} {right_str}"));
        }
        sel::BRACE => {
            let main = child_latex(arena, idx, 0);
            let left = child_latex(arena, idx, 1);
            let right = child_latex(arena, idx, 2);
            let right_str = if right.is_empty() { ".".to_string() } else { format!(" {right}") };
            out.push_str(&format!(
                "\\left {left} \\begin{{array}}{{l}} {main} \\end{{array}} \\right{right_str}"
            ));
        }
        sel::ROOT => {
            let main = child_latex(arena, idx, 0);
            let rad = child_latex(arena, idx, 1);
            out.push_str(&format!("\\sqrt[{rad}]{{ {main} }}"));
        }
        sel::FRACT => {
            // v3 inserts a transparent size/FULL marker between the numerator
            // and denominator slot LINEs; take the first two real LINE children
            // so the denominator is not swallowed by the marker.
            let slots = slot_lines(arena, idx);
            let num = slots.first().map_or_else(String::new, |&c| render_node(arena, c));
            let den = slots.get(1).map_or_else(String::new, |&c| render_node(arena, c));
            out.push_str(&format!("\\frac {{ {num} }} {{ {den} }}"));
        }
        sel::UBAR => {
            let main = child_latex(arena, idx, 0);
            out.push_str(&format!(" {{\\underline{{ {main} }} }} "));
        }
        sel::OBAR_T => {
            let main = child_latex(arena, idx, 0);
            out.push_str(&format!(" {{\\overline{{ {main} }} }} "));
        }
        sel::SUM | sel::PROD | sel::COPROD | sel::UNION | sel::INTER | sel::INTOP | sel::SUMOP => {
            let main = child_latex(arena, idx, 0);
            let lower = child_latex(arena, idx, 1);
            let upper = child_latex(arena, idx, 2);
            let op = big_op(s);
            let lower_str =
                if lower.is_empty() { String::new() } else { format!(" \\limits_{{ {lower} }}") };
            let upper_str = if upper.is_empty() { String::new() } else { format!("^ {upper}") };
            out.push_str(&format!("{op}{lower_str}{upper_str} {{ {main} }}"));
        }
        sel::INTEG => {
            let main = child_latex(arena, idx, 0);
            let lower = child_latex(arena, idx, 1);
            let upper = child_latex(arena, idx, 2);
            let (n, op) = integ_op(v);
            let lower_str =
                if lower.is_empty() { String::new() } else { format!(" \\limits_{{ {lower} }}") };
            let upper_str = if upper.is_empty() { String::new() } else { format!("^ {upper}") };
            out.push_str(&format!("{}\\{} {lower_str}{upper_str} {{ {main} }}", n, op));
        }
        sel::LIM => {
            let main = child_latex(arena, idx, 0);
            let lower = child_latex(arena, idx, 1);
            let upper = child_latex(arena, idx, 2);
            let lower_str =
                if lower.is_empty() { String::new() } else { format!(" \\limits_{{ {lower} }}") };
            out.push_str(&format!("\\mathop {{ {main} }}{lower_str}"));
        }
        sel::SUP => {
            let embell = arena[idx].children.first().map_or(false, |&c| is_size(arena[c].tag));
            let (base, sup) = if embell {
                (String::new(), script_operand(arena, idx))
            } else {
                (child_latex(arena, idx, 0), child_latex(arena, idx, 1))
            };
            if !sup.is_empty() {
                out.push_str(&format!("^{{ {sup} }}"));
            }
            if !base.is_empty() {
                out.push_str(&format!(" {{ {base} }}"));
            }
        }
        sel::SUB => {
            let embell = arena[idx].children.first().map_or(false, |&c| is_size(arena[c].tag));
            let (base, sub) = if embell {
                (String::new(), script_operand(arena, idx))
            } else {
                (child_latex(arena, idx, 0), child_latex(arena, idx, 1))
            };
            if !sub.is_empty() {
                out.push_str(&format!("_{{ {sub} }}"));
            }
            if !base.is_empty() {
                out.push_str(&format!(" {{ {base} }}"));
            }
        }
        sel::SUBSUP => {
            let embell = arena[idx].children.first().map_or(false, |&c| is_size(arena[c].tag));
            let (sub, sup) = if embell {
                subsup_operands(arena, idx)
            } else {
                (child_latex(arena, idx, 1), child_latex(arena, idx, 2))
            };
            if !sub.is_empty() {
                out.push_str(&format!("_{{ {sub} }}"));
            }
            if !sup.is_empty() {
                out.push_str(&format!("^{{ {sup} }}"));
            }
        }
        sel::VEC | sel::HAT | sel::TILDE | sel::ARC | sel::JSTATUS => {
            let main = child_latex(arena, idx, 0);
            let acc = accent(s, v);
            out.push_str(&format!("\\{acc}{{ {main} }}"));
        }
        sel::STRIKE => {
            let main = child_latex(arena, idx, 0);
            out.push_str(&format!("\\cancel{{ {main} }}"));
        }
        sel::BOX => {
            let main = child_latex(arena, idx, 0);
            out.push_str(&format!("\\boxed{{ {main} }}"));
        }
        sel::HBRACE | sel::HBRACK => {
            let main = child_latex(arena, idx, 0);
            let kind = if s == sel::HBRACE { "brace" } else { "brack" };
            out.push_str(&format!("\\over{kind}{{ {main} }}"));
        }
        sel::DIRAC => {
            let main = child_latex(arena, idx, 0);
            out.push_str(&format!("\\langle {main} \\rangle"));
        }
        sel::LDIV => {
            let main = child_latex(arena, idx, 0);
            out.push_str(&format!("\\overline{{ {main} }}"));
        }
        _ => {
            for &c in &arena[idx].children {
                latex(arena, c, out);
            }
        }
    }
}

fn big_op(s: u8) -> &'static str {
    match s {
        sel::SUM | sel::SUMOP => "\\sum",
        sel::PROD => "\\prod",
        sel::COPROD => "\\coprod",
        sel::UNION => "\\bigcup",
        sel::INTER => "\\bigcap",
        sel::INTOP => "\\intop",
        _ => "\\sum",
    }
}

fn integ_op(v: u16) -> (&'static str, &'static str) {
    match v & 0x000f {
        0x0001 => ("", "int"),
        0x0002 => ("", "iint"),
        0x0003 => ("", "iiint"),
        0x0004 => ("\\oint", "dots"),
        _ => ("", "int"),
    }
}

fn accent(s: u8, v: u16) -> &'static str {
    match s {
        sel::HAT => "\\hat",
        sel::TILDE => "\\widetilde",
        sel::ARC => "\\arc",
        sel::JSTATUS => "\\widetilde",
        sel::VEC => {
            if v & 0x0008 != 0 {
                "\\overrightharpoon"
            } else if v & 0x0002 != 0 {
                "\\overrightarrow"
            } else {
                "\\vec"
            }
        }
        _ => "\\hat",
    }
}

fn embell(arena: &[Node], idx: usize, out: &mut String) {
    let main = child_latex(arena, idx, 0);
    match arena[idx].embell {
        emb::D1PRIME | emb::R1ARROW => out.push_str(&format!("'{main}")),
        emb::D2PRIME | emb::BARROW => out.push_str(&format!("''{main}")),
        emb::D3PRIME | emb::RARROW => out.push_str(&format!("'''{main}")),
        emb::D1DOT => out.push_str(&format!("\\dot{{ {main} }}")),
        emb::D2DOT => out.push_str(&format!("\\ddot{{ {main} }}")),
        emb::D3DOT => out.push_str(&format!("\\dddot{{ {main} }}")),
        emb::TILDE => out.push_str(&format!("\\tilde{{ {main} }}")),
        emb::HAT => out.push_str(&format!("\\hat{{ {main} }}")),
        emb::NOT => out.push_str(&format!("\\neg {{ {main} }}")),
        emb::LARROW => out.push_str(&format!("\\overleftarrow{{ {main} }}")),
        emb::MBAR => out.push_str(&format!("\\bar{{ {main} }}")),
        emb::OBAR | emb::DBAR => out.push_str(&format!("\\bar{{ {main} }}")),
        emb::FROWN => out.push_str(&format!("\\frown{{ {main} }}")),
        emb::SMILE => out.push_str(&format!("\\smile{{ {main} }}")),
        emb::X_BARS => out.push_str(&format!("\\bar{{ {main} }}")),
        emb::UP_BAR => out.push_str(&format!("\\bar{{ {main} }}")),
        emb::DOWN_BAR => out.push_str(&format!("\\underline{{ {main} }}")),
        emb::U_BAR => out.push_str(&format!("\\bar{{ {main} }}")),
        emb::U_TILDE => out.push_str(&format!("\\tilde{{ {main} }}")),
        _ => out.push_str(&main),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Corpus regression test over real Equation.3 (MTEF v3) and MathType
    /// (MTEF v5) objects.
    ///
    /// Point `ANYDOC_MTEF_CORPUS` at a directory of raw `Equation Native`
    /// streams (the bytes *inside* the OLE object - exactly what
    /// `decode_stream` consumes). Skipped when the variable is unset, so the
    /// default suite stays hermetic.
    #[test]
    fn corpus_streams_decode() {
        let dir = match std::env::var_os("ANYDOC_MTEF_CORPUS") {
            Some(d) => PathBuf::from(d),
            None => return,
        };
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("read corpus dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |x| x == "mtef"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "corpus dir has no .mtef files");

        let mut failed: Vec<String> = Vec::new();
        for p in &paths {
            let data = std::fs::read(p).expect("read stream");
            let res = decode_stream(&data);
            eprintln!("{} -> {:?}", p.file_name().unwrap().to_string_lossy(), res);
            match res {
                Some(latex) if !latex.trim().is_empty() => {}
                other => failed.push(format!(
                    "{}: {:?}",
                    p.file_name().unwrap().to_string_lossy(),
                    other
                )),
            }
        }
        assert!(
            failed.is_empty(),
            "{}/{} streams failed to decode:\n{}",
            failed.len(),
            paths.len(),
            failed.join("\n")
        );
    }
}
