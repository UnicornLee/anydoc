//! Dump the record sequence of an Equation 3.0 / MathType OLE object, to
//! understand its MTEF framing before writing a decoder.
//!
//! `cargo run --example mtef_dump -- <oleObject1.bin>`

use std::io::Cursor;
use std::io::Read;

const OLE_CB_HDR: usize = 28;

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum Tag {
    End = 0,
    Line = 1,
    Char = 2,
    Tmpl = 3,
    Pile = 4,
    Matrix = 5,
    Embell = 6,
    Ruler = 7,
    FontStyleDef = 8,
    Size = 9,
    Full = 10,
    Sub = 11,
    Sub2 = 12,
    Sym = 13,
    SubSym = 14,
    Color = 15,
    ColorDef = 16,
    FontDef = 17,
    EqnPrefs = 18,
    EncodingDef = 19,
    Future = 100,
}

fn tag_name(t: u8) -> &'static str {
    match t {
        0 => "END",
        1 => "LINE",
        2 => "CHAR",
        3 => "TMPL",
        4 => "PILE",
        5 => "MATRIX",
        6 => "EMBELL",
        7 => "RULER",
        8 => "FONTSTYLEDEF",
        9 => "SIZE",
        10 => "FULL",
        11 => "SUB",
        12 => "SUB2",
        13 => "SYM",
        14 => "SUBSYM",
        15 => "COLOR",
        16 => "COLORDEF",
        17 => "FONTDEF",
        18 => "EQNPRF",
        19 => "ENCDEF",
        _ => "FUTURE",
    }
}

struct R<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        R { b, pos: 0 }
    }
    fn u8(&mut self) -> u8 {
        if self.pos >= self.b.len() {
            return 0;
        }
        let v = self.b[self.pos];
        self.pos += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        let v = u16::from_le_bytes([self.b[self.pos], self.b[self.pos + 1]]);
        self.pos += 2;
        v
    }
    fn eof(&self) -> bool {
        self.pos >= self.b.len()
    }
}

fn main() {
    let path = std::env::args_os().nth(1).expect("usage: mtef_dump <oleObject1.bin>");
    let bytes = std::fs::read(&path).expect("read");
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).expect("cfb");
    let entry = cf
        .read_root_storage()
        .find(|e| e.name().contains("Equation Native"))
        .expect("Equation Native stream");
    let name = entry.name().to_string();
    let mut stream = cf.open_stream(&name).expect("open stream");
    let mut data = Vec::new();
    stream.read_to_end(&mut data).expect("read");

    let hdr = &data[..OLE_CB_HDR.min(data.len())];
    let cb_hdr = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
    let cb_size = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    println!("cbHdr={cb_hdr} cbSize={cb_size} streamLen={}", data.len());
    let start = cb_hdr.min(data.len());
    let end = (start + cb_size).min(data.len());
    let body = &data[start..end];

    let mut r = R::new(body);
    let _ver = r.u8();
    let _plat = r.u8();
    let _prod = r.u8();
    let _v = r.u8();
    let _sub = r.u8();
    // application name (null terminated)
    while !r.eof() && r.b[r.pos] != 0 {
        r.pos += 1;
    }
    if !r.eof() {
        r.pos += 1;
    }

    println!("records:");
    while !r.eof() {
        let tag = r.u8();
        match tag {
            0 => {
                println!("  END");
                break;
            }
            1 => {
                // LINE
                let opts = r.u8();
                if opts & 0x08 != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                if opts & 0x04 != 0 {
                    let _ = r.u8();
                }
                if opts & 0x02 != 0 {
                    let n = r.u8();
                    for _ in 0..n {
                        let _ = r.u8();
                        let _ = r.u16();
                    }
                }
                let null = opts & 0x01 != 0;
                println!("  LINE null={null} opts={opts:#x}");
            }
            2 => {
                // CHAR
                let opts = r.u8();
                if opts & 0x08 != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let _tf = r.u8();
                let mut mtcode = 0u16;
                if opts & 0x20 == 0 {
                    mtcode = r.u16();
                }
                if opts & 0x04 != 0 {
                    let _ = r.u8();
                }
                if opts & 0x10 != 0 {
                    let _ = r.u16();
                }
                println!("  CHAR mtcode={mtcode:#06x} opts={opts:#x}");
            }
            3 => {
                // TMPL
                let opts = r.u8();
                if opts & 0x08 != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let sel = r.u8();
                let b1 = r.u8();
                let variation = if b1 & 0x80 != 0 {
                    let b2 = r.u8();
                    ((b1 & 0x7f) as u16) | ((b2 as u16) << 8)
                } else {
                    b1 as u16
                };
                let _topts = r.u8();
                println!("  TMPL selector={sel} variation={variation:#x} opts={opts:#x}");
            }
            4 => {
                let opts = r.u8();
                if opts & 0x08 != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let _h = r.u8();
                let _v = r.u8();
                println!("  PILE opts={opts:#x}");
            }
            5 => {
                let opts = r.u8();
                if opts & 0x08 != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let _val = r.u8();
                let _h = r.u8();
                let _v = r.u8();
                let rows = r.u8();
                let cols = r.u8();
                println!("  MATRIX {rows}x{cols}");
            }
            6 => {
                let opts = r.u8();
                if opts & 0x08 != 0 {
                    let _ = r.u16();
                    let _ = r.u16();
                }
                let et = r.u8();
                println!("  EMBELL type={et}");
            }
            8 => {
                let _idx = r.u8();
                while !r.eof() && r.b[r.pos] != 0 {
                    r.pos += 1;
                }
                if !r.eof() {
                    r.pos += 1;
                }
                println!("  FONTSTYLEDEF");
            }
            9 => {
                let _l = r.u8();
                let _d = r.u8();
                println!("  SIZE");
            }
            11 | 12 | 13 | 14 => {
                println!("  {}", tag_name(tag));
            }
            17 => {
                let _idx = r.u8();
                while !r.eof() && r.b[r.pos] != 0 {
                    r.pos += 1;
                }
                if !r.eof() {
                    r.pos += 1;
                }
                println!("  FONTDEF");
            }
            15 => {
                let _idx = r.u8();
                println!("  COLOR");
            }
            16 => {
                let opts = r.u8();
                if opts & 0x01 != 0 {
                    for _ in 0..4 {
                        let _ = r.u16();
                    }
                } else {
                    for _ in 0..3 {
                        let _ = r.u16();
                    }
                }
                if opts & 0x04 != 0 {
                    while !r.eof() && r.b[r.pos] != 0 {
                        r.pos += 1;
                    }
                    if !r.eof() {
                        r.pos += 1;
                    }
                }
                println!("  COLORDEF");
            }
            10 => println!("  FULL"),
            18 => {
                let _o = r.u8();
                let n = r.u8();
                for _ in 0..n {
                    let _ = r.u8();
                }
                let n2 = r.u8();
                for _ in 0..n2 {
                    let _ = r.u8();
                }
                let n3 = r.u8();
                for _ in 0..n3 {
                    let c = r.u8();
                    if c != 0 {
                        let _ = r.u8();
                    }
                }
                println!("  EQNPRF");
            }
            19 => {
                while !r.eof() && r.b[r.pos] != 0 {
                    r.pos += 1;
                }
                if !r.eof() {
                    r.pos += 1;
                }
                println!("  ENCDEF");
            }
            t if t >= 100 => {
                let skip = r.u8();
                r.pos += skip as usize;
                println!("  FUTURE skip={skip}");
            }
            t => println!("  ??? tag={t}"),
        }
    }
}
