//! Rasterises the actor's POV frame to a self-contained JPEG.
//!
//! Same projection as the SVG backend (`frame::project`), same neutral gestalt —
//! a backdrop, depth-shaded discs, their identity labels, a crosshair, and the
//! caption — drawn into a pixel buffer so an MCP client that only renders raster
//! images can still see what the actor sees.
//!
//! Zero dependencies on purpose (this is a bootstrap binary): a tiny 5x7 bitmap
//! font and a from-scratch baseline JPEG encoder (4:4:4, standard quantisation +
//! Huffman tables, naïve FDCT). JPEG so the frame stays small on the wire.

use crate::frame::{self, HEIGHT, WIDTH};
use universe_supervisor::perception::{Pov, SphereSighting};

type Rgb = [u8; 3];

const BACKDROP: Rgb = [0x0b, 0x0e, 0x14];
const GROUND: Rgb = [0x11, 0x16, 0x1f];
const STROKE: Rgb = [0xe6, 0xed, 0xf3];
const HAIR: Rgb = [0x7d, 0x85, 0x90];

/// A width×height RGB canvas with just the primitives the frame needs.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>, // row-major RGB
}

impl Canvas {
    fn new(w: usize, h: usize, fill: Rgb) -> Self {
        let mut px = Vec::with_capacity(w * h * 3);
        for _ in 0..w * h {
            px.extend_from_slice(&fill);
        }
        Self { w, h, px }
    }

    fn put(&mut self, x: i64, y: i64, c: Rgb) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        self.px[i] = c[0];
        self.px[i + 1] = c[1];
        self.px[i + 2] = c[2];
    }

    /// Alpha-blends `c` over the existing pixel (`a` in 0..=255).
    fn blend(&mut self, x: i64, y: i64, c: Rgb, a: u32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        for (k, &src) in c.iter().enumerate() {
            let dst = self.px[i + k] as u32;
            self.px[i + k] = ((src as u32 * a + dst * (255 - a)) / 255) as u8;
        }
    }

    fn fill_rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgb) {
        for y in y0.max(0)..y1.min(self.h as i64) {
            for x in x0.max(0)..x1.min(self.w as i64) {
                self.put(x, y, c);
            }
        }
    }

    /// A filled disc with a 1px stroke ring, blended so overlaps read as depth.
    fn disc(&mut self, cx: f64, cy: f64, r: f64, fill: Rgb, alpha: u32) {
        let min_x = (cx - r - 1.0).floor() as i64;
        let max_x = (cx + r + 1.0).ceil() as i64;
        let min_y = (cy - r - 1.0).floor() as i64;
        let max_y = (cy + r + 1.0).ceil() as i64;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                if d <= r - 1.0 {
                    self.blend(x, y, fill, alpha);
                } else if d <= r + 0.5 {
                    self.put(x, y, STROKE); // the ring reads crisp, like the SVG
                }
            }
        }
    }

    /// A 1px axis-aligned segment (the frame only ever needs the crosshair).
    fn segment(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgb) {
        if y0 == y1 {
            for x in x0.min(x1)..=x0.max(x1) {
                self.put(x, y0, c);
            }
        } else if x0 == x1 {
            for y in y0.min(y1)..=y0.max(y1) {
                self.put(x0, y, c);
            }
        }
    }

    /// Draws a string with the 5x7 font, top-left at (x, y). Unknown glyphs are
    /// skipped but still advance, so spacing stays stable.
    fn text(&mut self, x: i64, y: i64, s: &str, c: Rgb) {
        let mut pen = x;
        for ch in s.chars() {
            if let Some(glyph) = glyph(ch) {
                for (row, bits) in glyph.iter().enumerate() {
                    for col in 0..5 {
                        if bits & (0x10 >> col) != 0 {
                            self.put(pen + col as i64, y + row as i64, c);
                        }
                    }
                }
            }
            pen += 6; // 5px glyph + 1px gap
        }
    }
}

/// Text width in pixels for centring labels (matches `Canvas::text` advance).
fn text_width(s: &str) -> i64 {
    s.chars().count() as i64 * 6
}

/// Renders the POV frame to JPEG bytes — the raster twin of `frame::render_svg`.
pub fn render_jpeg(pov: &Pov, sightings: &[SphereSighting], caption: &str) -> Vec<u8> {
    let w = WIDTH as usize;
    let h = HEIGHT as usize;
    let cx = WIDTH / 2.0;
    let cy = HEIGHT / 2.0;

    let mut canvas = Canvas::new(w, h, BACKDROP);
    // Ground/void backdrop — a neutral horizon so the frame reads as a scene.
    canvas.fill_rect(0, cy as i64, w as i64, h as i64, GROUND);

    for d in frame::project(pov, sightings) {
        let shade = d.shade as u8;
        let fill: Rgb = [shade, shade, (d.shade + 40).min(255) as u8];
        canvas.disc(d.x, d.y, d.r, fill, 217); // ~0.85 opacity, as the SVG
        if d.r >= 8.0 {
            let label = ascii_fold(&d.label);
            let lx = (d.x as i64) - text_width(&label) / 2;
            let ly = (d.y - d.r - 3.0) as i64 - 7;
            canvas.text(lx, ly, &label, STROKE);
        }
    }

    // Crosshair at the look direction, and the honesty caption.
    canvas.segment(cx as i64 - 6, cy as i64, cx as i64 + 6, cy as i64, HAIR);
    canvas.segment(cx as i64, cy as i64 - 6, cx as i64, cy as i64 + 6, HAIR);
    canvas.text(8, 8, &ascii_fold(caption), HAIR);

    encode_jpeg(w, h, &canvas.px, 82)
}

/// The 5x7 font is uppercase-only + digits + a little punctuation; fold the rest
/// so canonical ids (lowercase, `:`/`-`/`/`) stay legible in the raster.
fn ascii_fold(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c })
        .collect()
}

// --- baseline JPEG encoder (4:4:4) -----------------------------------------
//
// Standard sequential DCT-based JPEG (ITU-T T.81 baseline): SOI, APP0/JFIF, two
// DQT tables, SOF0, four DHT tables, SOS + entropy-coded segment, EOI. No chroma
// subsampling (each component sampled 1x1, so one 8x8 block per component per
// MCU). WIDTH/HEIGHT are multiples of 8, so no edge padding is needed.

/// Base luminance quantisation table (T.81 Annex K.1), natural (row) order.
const QUANT_LUMA: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
/// Base chrominance quantisation table (T.81 Annex K.1), natural order.
const QUANT_CHROMA: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// The zig-zag scan order (natural index for each of the 64 zig-zag positions).
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

// Standard Huffman table specs (T.81 Annex K.3): `bits[i]` = number of codes of
// length i+1; `vals` = the symbols, in canonical order.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_LUMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_CHROMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// A canonical Huffman code table: `(code, length)` indexed by symbol value.
struct Huff {
    codes: [(u16, u8); 256],
}

impl Huff {
    fn new(bits: &[u8; 16], vals: &[u8]) -> Self {
        let mut codes = [(0u16, 0u8); 256];
        let mut code: u16 = 0;
        let mut k = 0;
        for len in 1..=16u8 {
            for _ in 0..bits[(len - 1) as usize] {
                codes[vals[k] as usize] = (code, len);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        Self { codes }
    }
}

/// Scales a base quantisation table to a quality in 1..=100 (T.81 Annex K).
fn scale_quant(base: &[u8; 64], quality: u32) -> [u16; 64] {
    let q = quality.clamp(1, 100);
    let s = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut out = [0u16; 64];
    for i in 0..64 {
        let v = (base[i] as u32 * s + 50) / 100;
        out[i] = v.clamp(1, 255) as u16;
    }
    out
}

/// MSB-first bit sink with the mandatory `0xFF -> 0xFF 0x00` byte stuffing.
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self { out: Vec::new(), acc: 0, nbits: 0 }
    }
    fn put(&mut self, code: u16, size: u8) {
        if size == 0 {
            return;
        }
        self.acc = (self.acc << size) | (code as u32 & ((1u32 << size) - 1));
        self.nbits += size as u32;
        while self.nbits >= 8 {
            let byte = ((self.acc >> (self.nbits - 8)) & 0xff) as u8;
            self.out.push(byte);
            if byte == 0xff {
                self.out.push(0x00);
            }
            self.nbits -= 8;
        }
    }
    /// Pads the final partial byte with 1-bits, as the standard requires.
    fn flush(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            self.put((1u16 << pad) - 1, pad as u8);
        }
    }
}

/// Bit-length category of a signed coefficient, and its mantissa bits.
fn category(v: i32) -> (u8, u16) {
    if v == 0 {
        return (0, 0);
    }
    let a = v.unsigned_abs();
    let size = 32 - a.leading_zeros();
    // Negative values use the one's-complement (v - 1) mantissa.
    let mantissa = if v > 0 { v } else { v - 1 } as u16 & ((1u16 << size) - 1);
    (size as u8, mantissa)
}

/// Naïve separable forward DCT over an 8x8 block (level-shifted by -128).
fn fdct(block: &[f32; 64]) -> [f32; 64] {
    // Precomputed cosine basis: COS[u][x] = cos((2x+1) u pi / 16).
    let mut cos = [[0.0f32; 8]; 8];
    for (u, row) in cos.iter_mut().enumerate() {
        for (x, c) in row.iter_mut().enumerate() {
            *c = (((2 * x + 1) as f32) * (u as f32) * std::f32::consts::PI / 16.0).cos();
        }
    }
    let alpha = |u: usize| if u == 0 { (0.5f32).sqrt() } else { 1.0 };
    let mut out = [0.0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut sum = 0.0f32;
            for y in 0..8 {
                for x in 0..8 {
                    sum += block[y * 8 + x] * cos[u][x] * cos[v][y];
                }
            }
            out[v * 8 + u] = 0.25 * alpha(u) * alpha(v) * sum;
        }
    }
    out
}

/// Encodes one 8x8 block: quantise, zig-zag, DC-differential + AC run-length
/// into `bw`. Returns the block's DC for the next block's differential.
fn encode_block(
    bw: &mut BitWriter,
    pixels: &[f32; 64],
    quant: &[u16; 64],
    dc_huff: &Huff,
    ac_huff: &Huff,
    prev_dc: i32,
) -> i32 {
    let dct = fdct(pixels);
    let mut q = [0i32; 64];
    for i in 0..64 {
        let f = dct[i] / quant[i] as f32;
        q[i] = (f + if f >= 0.0 { 0.5 } else { -0.5 }) as i32; // round to nearest
    }
    // Zig-zag reorder.
    let mut zz = [0i32; 64];
    for i in 0..64 {
        zz[i] = q[ZIGZAG[i]];
    }

    // DC: differential from the previous block of the same component.
    let diff = zz[0] - prev_dc;
    let (size, mant) = category(diff);
    bw.put(dc_huff.codes[size as usize].0, dc_huff.codes[size as usize].1);
    bw.put(mant, size);

    // AC: run-length of zeros, ZRL (0xF0) per 16 zeros, EOB (0x00) if trailing.
    let mut run = 0;
    for &coeff in zz.iter().take(64).skip(1) {
        if coeff == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            bw.put(ac_huff.codes[0xf0].0, ac_huff.codes[0xf0].1);
            run -= 16;
        }
        let (s, m) = category(coeff);
        let sym = ((run << 4) | s as usize) & 0xff;
        bw.put(ac_huff.codes[sym].0, ac_huff.codes[sym].1);
        bw.put(m, s);
        run = 0;
    }
    if run > 0 {
        bw.put(ac_huff.codes[0].0, ac_huff.codes[0].1); // EOB
    }
    zz[0]
}

fn encode_jpeg(w: usize, h: usize, rgb: &[u8], quality: u32) -> Vec<u8> {
    let q_luma = scale_quant(&QUANT_LUMA, quality);
    let q_chroma = scale_quant(&QUANT_CHROMA, quality);

    // RGB -> YCbCr (JFIF), level-shifted by -128, as full planes.
    let n = w * h;
    let mut yy = vec![0.0f32; n];
    let mut cb = vec![0.0f32; n];
    let mut cr = vec![0.0f32; n];
    for i in 0..n {
        let r = rgb[i * 3] as f32;
        let g = rgb[i * 3 + 1] as f32;
        let b = rgb[i * 3 + 2] as f32;
        yy[i] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
        cb[i] = -0.168736 * r - 0.331264 * g + 0.5 * b;
        cr[i] = 0.5 * r - 0.418688 * g - 0.081312 * b;
    }

    let dc_luma = Huff::new(&DC_LUMA_BITS, &DC_LUMA_VALS);
    let ac_luma = Huff::new(&AC_LUMA_BITS, &AC_LUMA_VALS);
    let dc_chroma = Huff::new(&DC_CHROMA_BITS, &DC_CHROMA_VALS);
    let ac_chroma = Huff::new(&AC_CHROMA_BITS, &AC_CHROMA_VALS);

    // --- headers ---
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&[0xff, 0xd8]); // SOI
    // APP0 / JFIF.
    out.extend_from_slice(&[0xff, 0xe0, 0x00, 0x10]);
    out.extend_from_slice(b"JFIF\0");
    out.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    // DQT (luma id 0, chroma id 1), each written in zig-zag order.
    for (id, table) in [(0u8, &q_luma), (1u8, &q_chroma)] {
        out.extend_from_slice(&[0xff, 0xdb, 0x00, 0x43, id]);
        for i in 0..64 {
            out.push(table[ZIGZAG[i]] as u8);
        }
    }
    // SOF0: 3 components, 4:4:4 (all sampled 1x1).
    out.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08]);
    out.extend_from_slice(&(h as u16).to_be_bytes());
    out.extend_from_slice(&(w as u16).to_be_bytes());
    out.push(0x03);
    out.extend_from_slice(&[0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    // DHT: the four standard tables (class<<4|id: DC0,AC0,DC1,AC1).
    for (tc_th, bits, vals) in [
        (0x00u8, &DC_LUMA_BITS[..], &DC_LUMA_VALS[..]),
        (0x10, &AC_LUMA_BITS[..], &AC_LUMA_VALS[..]),
        (0x01, &DC_CHROMA_BITS[..], &DC_CHROMA_VALS[..]),
        (0x11, &AC_CHROMA_BITS[..], &AC_CHROMA_VALS[..]),
    ] {
        let len = 2 + 1 + 16 + vals.len();
        out.extend_from_slice(&[0xff, 0xc4]);
        out.extend_from_slice(&(len as u16).to_be_bytes());
        out.push(tc_th);
        out.extend_from_slice(bits);
        out.extend_from_slice(vals);
    }
    // SOS: 3 components, DC/AC selectors (Y->0/0, Cb,Cr->1/1).
    out.extend_from_slice(&[0xff, 0xda, 0x00, 0x0c, 0x03]);
    out.extend_from_slice(&[0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00]);

    // --- entropy-coded segment: interleaved MCUs, one 8x8 block per component ---
    let mut bw = BitWriter::new();
    let (mut pdc_y, mut pdc_cb, mut pdc_cr) = (0i32, 0i32, 0i32);
    let extract = |plane: &[f32], bx: usize, by: usize| -> [f32; 64] {
        let mut blk = [0.0f32; 64];
        for row in 0..8 {
            for col in 0..8 {
                blk[row * 8 + col] = plane[(by + row) * w + (bx + col)];
            }
        }
        blk
    };
    for by in (0..h).step_by(8) {
        for bx in (0..w).step_by(8) {
            pdc_y = encode_block(&mut bw, &extract(&yy, bx, by), &q_luma, &dc_luma, &ac_luma, pdc_y);
            pdc_cb =
                encode_block(&mut bw, &extract(&cb, bx, by), &q_chroma, &dc_chroma, &ac_chroma, pdc_cb);
            pdc_cr =
                encode_block(&mut bw, &extract(&cr, bx, by), &q_chroma, &dc_chroma, &ac_chroma, pdc_cr);
        }
    }
    bw.flush();
    out.extend_from_slice(&bw.out);
    out.extend_from_slice(&[0xff, 0xd9]); // EOI
    out
}

// --- base64 (RFC 4648, `=` padding) ----------------------------------------

/// Standard base64 so the PNG can ride in an MCP image content block without a
/// dependency. Shared by the adapter's tool-result assembly.
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let t = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((t >> 18) & 0x3f) as usize] as char);
        out.push(A[((t >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { A[((t >> 6) & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[(t & 0x3f) as usize] as char } else { '=' });
    }
    out
}

// --- 5x7 bitmap font -------------------------------------------------------

/// Rows top-to-bottom; the low 5 bits are columns, `0x10` = leftmost.
fn glyph(ch: char) -> Option<[u8; 7]> {
    let g: [u8; 7] = match ch {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        ':' => [0x00, 0x04, 0x04, 0x00, 0x04, 0x04, 0x00],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        _ => return None,
    };
    Some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pov() -> Pov {
        Pov {
            actor: "root".into(),
            generated: false,
            eye: [0.0, 0.0, 0.0],
            eye_source: "situated",
            look_at: [0.0, 0.0, -1.0],
            yaw: 0.0,
            pitch: 0.0,
            projection: "physics_sphere",
        }
    }

    fn sighting(label: &str, position: [f64; 3]) -> SphereSighting {
        SphereSighting {
            key: "k".into(),
            label: label.into(),
            primitive: "sphere",
            position,
            distance_m: universe_supervisor::perception::pov::distance([0.0, 0.0, 0.0], position),
            bearing: "ahead",
        }
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn huffman_codes_are_canonical() {
        // DC luma: 12 symbols, first code length 2 (bits = [0,1,5,...]). Symbol 0
        // is the single length-2 code `00`; symbol 1 opens the length-3 run.
        let h = Huff::new(&DC_LUMA_BITS, &DC_LUMA_VALS);
        assert_eq!(h.codes[0], (0b00, 2));
        assert_eq!(h.codes[1], (0b010, 3));
    }

    #[test]
    fn category_encodes_sign_with_ones_complement_mantissa() {
        assert_eq!(category(0), (0, 0));
        assert_eq!(category(1), (1, 0b1));
        assert_eq!(category(-1), (1, 0b0));
        assert_eq!(category(2), (2, 0b10));
        assert_eq!(category(-2), (2, 0b01));
        assert_eq!(category(5), (3, 0b101));
        assert_eq!(category(-5), (3, 0b010));
    }

    #[test]
    fn render_jpeg_is_framed_by_soi_and_eoi() {
        let jpg = render_jpeg(&pov(), &[sighting("balise-zero", [0.0, 0.0, -5.0])], "test rev 1");
        assert_eq!(&jpg[..2], &[0xff, 0xd8]); // SOI
        assert_eq!(&jpg[jpg.len() - 2..], &[0xff, 0xd9]); // EOI
        // A JFIF APP0 marker follows the SOI.
        assert_eq!(&jpg[2..4], &[0xff, 0xe0]);
        assert_eq!(&jpg[6..11], b"JFIF\0");
        // Far smaller than the raw 640x360x3 pixels: it really compresses.
        assert!(jpg.len() < WIDTH as usize * HEIGHT as usize * 3);
    }

    #[test]
    fn an_empty_frame_still_encodes() {
        // No sightings: just backdrop + crosshair + caption, still a valid JPEG.
        let jpg = render_jpeg(&pov(), &[], "empty");
        assert_eq!(&jpg[..2], &[0xff, 0xd8]);
        assert_eq!(&jpg[jpg.len() - 2..], &[0xff, 0xd9]);
    }
}
