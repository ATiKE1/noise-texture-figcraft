//! A small, fast PNG encoder and base64, which is how FigCraft takes image bytes from a plugin.
//!
//! FigCraft runs plugins in an interpreter with an instruction budget, so this encoder is built to
//! spend few instructions per byte rather than to squeeze out the last byte:
//!
//! - an image of at most 256 colours (most textures: one colour at a few opacities, or two colours)
//!   is written with a palette at 1, 2, 4 or 8 bits per pixel, which is up to 32 times less data
//!   than RGBA; the palette is sorted, so smooth textures stay smooth in their indices;
//! - each row gets the cheapest of the None, Sub and Up filters, judged in one pass;
//! - the zlib stream is a single deflate block with the fixed Huffman codes and greedy LZ77
//!   matching (one candidate per position), which catches the runs and repeats textures have.

/// The CRC-32 table (polynomial 0xEDB88320), built at compile time.
const CRC: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for part in parts {
        for b in *part {
            c = CRC[((c ^ u32::from(*b)) & 0xff) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xffff_ffff
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // 5552 bytes is the most that can be summed before the sums can overflow.
    for block in data.chunks(5552) {
        for x in block {
            a += u32::from(*x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Writes bits least significant first, as deflate packs them, collecting whole 32-bit words.
struct Bits {
    words: Vec<u32>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, bits: u32, len: u32) {
        self.acc |= u64::from(bits) << self.n;
        self.n += len;
        if self.n >= 32 {
            self.words.push(self.acc as u32);
            self.acc >>= 32;
            self.n -= 32;
        }
    }

    /// Append the bits written to `out`, the last byte padded with zeros.
    fn finish(self, out: &mut Vec<u8>) {
        out.extend(self.words.iter().flat_map(|w| w.to_le_bytes()));
        out.extend(self.acc.to_le_bytes().iter().take(self.n.div_ceil(8) as usize));
    }
}

/// `code` (`len` bits) reversed: Huffman codes are sent most significant bit first.
const fn reverse(code: u32, len: u32) -> u32 {
    let mut r = 0;
    let mut i = 0;
    while i < len {
        r |= ((code >> i) & 1) << (len - 1 - i);
        i += 1;
    }
    r
}

/// The fixed literal/length code (RFC 1951 §3.2.6): `(reversed code, bits)` for symbols 0–287.
const LITERALS: [(u32, u32); 288] = {
    let mut t = [(0u32, 0u32); 288];
    let mut s = 0;
    while s < 288 {
        let (code, len) = match s {
            0..=143 => (0x30 + s as u32, 8),
            144..=255 => (0x190 + (s as u32 - 144), 9),
            256..=279 => (s as u32 - 256, 7),
            _ => (0xc0 + (s as u32 - 280), 8),
        };
        t[s] = (reverse(code, len), len);
        s += 1;
    }
    t
};

const LENGTH_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LENGTH_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] =
    [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// The length code (index into the length tables) of each match length 3–258.
const LENGTH_CODE: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut code = 0;
    let mut len = 3;
    while len <= 258 {
        while code + 1 < 29 && LENGTH_BASE[code + 1] as usize <= len {
            code += 1;
        }
        t[len - 3] = code as u8;
        len += 1;
    }
    t
};

/// The distance code of distance `d` (1–32768).
fn dist_code(d: usize) -> usize {
    // Past 4, the codes come in pairs whose bases double: the top two bits of d − 1 pick it.
    if d <= 4 {
        return d - 1;
    }
    let x = (d - 1) as u32;
    let bit = 31 - x.leading_zeros();
    (bit as usize) * 2 + ((x >> (bit - 1)) & 1) as usize
}

const WINDOW: usize = 32_768;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;

/// `data` as a zlib stream: one fixed-Huffman deflate block with greedy LZ77 matches.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut bits = Bits { words: Vec::with_capacity(data.len() / 3 + 16), acc: 0, n: 0 };
    // The final block, compressed with the fixed codes.
    bits.put(1, 1);
    bits.put(1, 2);
    // The four bytes at `i` as a little-endian word (0 past the end).
    let word = |i: usize| data.get(i..i + 4).and_then(|w| <[u8; 4]>::try_from(w).ok()).map_or(0, u32::from_le_bytes);
    // The last position + 1 at which each hash of four bytes was seen (0: never).
    let mut head = vec![0u32; 1 << HASH_BITS];
    let mut i = 0;
    while i + 4 <= data.len() {
        let w = word(i);
        let h = (w.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize;
        let candidate = head[h] as usize;
        head[h] = (i + 1) as u32;
        if candidate == 0 || i + 1 - candidate > WINDOW || word(candidate - 1) != w {
            let (code, len) = LITERALS[usize::from(data[i])];
            bits.put(code, len);
            i += 1;
            continue;
        }
        let from = candidate - 1;
        let max = (data.len() - i).min(MAX_MATCH);
        let mut len = 4;
        while len + 4 <= max && word(from + len) == word(i + len) {
            len += 4;
        }
        while len < max && data[from + len] == data[i + len] {
            len += 1;
        }
        let code = usize::from(LENGTH_CODE[len - 3]);
        let (sym, sym_len) = LITERALS[257 + code];
        bits.put(sym, sym_len);
        bits.put((len - usize::from(LENGTH_BASE[code])) as u32, u32::from(LENGTH_EXTRA[code]));
        let d = i - from;
        let dc = dist_code(d);
        bits.put(reverse(dc as u32, 5), 5);
        bits.put((d - usize::from(DIST_BASE[dc])) as u32, u32::from(DIST_EXTRA[dc]));
        i += len;
    }
    // The last bytes: too few to match.
    for b in &data[i.min(data.len())..] {
        let (code, len) = LITERALS[usize::from(*b)];
        bits.put(code, len);
    }
    // End of block.
    let (code, len) = LITERALS[256];
    bits.put(code, len);
    // CMF/FLG: deflate with a 32 KB window, fastest compression, check bits.
    let mut out = Vec::with_capacity(bits.words.len() * 4 + 16);
    out.extend_from_slice(&[0x78, 0x01]);
    bits.finish(&mut out);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// The image's colours as a palette and each pixel's index into it, when there are at most 256.
/// The palette is sorted (by alpha, then colour), so a smooth texture has smooth indices.
fn palettize(pixels: &[u32]) -> Option<(Vec<u32>, Vec<u8>)> {
    const SLOTS: usize = 1024;
    // Open addressing on the RGBA value: `keys[slot]` holds colour + 1 (0 is empty).
    let mut keys = vec![0u64; SLOTS];
    let mut index = vec![0u8; SLOTS];
    let mut colours: Vec<u32> = Vec::with_capacity(256);
    let mut out = vec![0u8; pixels.len()];
    let (mut last, mut last_index) = (None, 0u8);
    for (o, &p) in out.iter_mut().zip(pixels) {
        if last == Some(p) {
            *o = last_index;
            continue;
        }
        let key = u64::from(p) + 1;
        let mut slot = (p.wrapping_mul(0x9e37_79b1) >> 22) as usize;
        let i = loop {
            if keys[slot] == key {
                break index[slot];
            }
            if keys[slot] == 0 {
                if colours.len() == 256 {
                    return None;
                }
                keys[slot] = key;
                index[slot] = colours.len() as u8;
                colours.push(p);
                break index[slot];
            }
            slot = (slot + 1) % SLOTS;
        };
        (last, last_index) = (Some(p), i);
        *o = i;
    }
    // Sort by alpha, then colour (pixels are RGBA bytes read little-endian: alpha is the top byte).
    let mut order: Vec<u8> = (0..colours.len()).map(|i| i as u8).collect();
    order.sort_by_key(|i| colours[*i as usize]);
    let mut remap = [0u8; 256];
    for (new, old) in order.iter().enumerate() {
        remap[*old as usize] = new as u8;
    }
    out.iter_mut().for_each(|i| *i = remap[*i as usize]);
    Some((order.iter().map(|i| colours[*i as usize]).collect(), out))
}

/// Append row `row` with the cheapest of the None, Sub and Up filters (judged by the sum of the
/// residuals as signed bytes) for `bpp` bytes per pixel, given the row above (zeros for the first).
fn filtered_row(raw: &mut Vec<u8>, row: &[u8], prev: &[u8], bpp: usize) {
    let (mut none, mut sub, mut up) = (0u32, 0u32, 0u32);
    for (i, (x, b)) in row.iter().zip(prev).enumerate() {
        let a = if i >= bpp { row[i - bpp] } else { 0 };
        none += u32::from((*x as i8).unsigned_abs());
        sub += u32::from((x.wrapping_sub(a) as i8).unsigned_abs());
        up += u32::from((x.wrapping_sub(*b) as i8).unsigned_abs());
    }
    if none <= sub && none <= up {
        raw.push(0);
        raw.extend_from_slice(row);
    } else if sub <= up {
        raw.push(1);
        raw.extend(row.iter().enumerate().map(|(i, x)| x.wrapping_sub(if i >= bpp { row[i - bpp] } else { 0 })));
    } else {
        raw.push(2);
        raw.extend(row.iter().zip(prev).map(|(x, b)| x.wrapping_sub(*b)));
    }
}

/// The scanlines of a palette image: indices packed `depth` bits each (most significant first),
/// unfiltered below 8 bits (packed indices aren't numbers to take differences of).
fn palette_scanlines(indices: &[u8], w: usize, depth: usize) -> Vec<u8> {
    let per_byte = 8 / depth;
    let stride = w.div_ceil(per_byte);
    let mut raw = Vec::with_capacity((stride + 1) * (indices.len() / w.max(1)));
    let mut prev = vec![0u8; stride];
    let mut packed = vec![0u8; stride];
    for row in indices.chunks_exact(w) {
        if depth == 8 {
            packed.copy_from_slice(row);
            filtered_row(&mut raw, &packed, &prev, 1);
            std::mem::swap(&mut prev, &mut packed);
        } else {
            packed.iter_mut().for_each(|b| *b = 0);
            for (x, i) in row.iter().enumerate() {
                packed[x / per_byte] |= i << (8 - depth * (x % per_byte + 1));
            }
            raw.push(0);
            raw.extend_from_slice(&packed);
        }
    }
    raw
}

/// `width` × `height` straight-alpha pixels (R, G, B, A bytes read little-endian, row by row) as
/// a PNG file. `None` when `pixels` has the wrong length or the image is empty.
pub fn encode(width: u32, height: u32, pixels: &[u32]) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || pixels.len() != w.checked_mul(h)? {
        return None;
    }
    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    let raw = match palettize(pixels) {
        Some((palette, indices)) => {
            let depth: usize = match palette.len() {
                0..=2 => 1,
                3..=4 => 2,
                5..=16 => 4,
                _ => 8,
            };
            // Bit depth, palette colour, deflate, adaptive filtering, no interlace.
            ihdr.extend_from_slice(&[depth as u8, 3, 0, 0, 0]);
            chunk(&mut png, b"IHDR", &ihdr);
            let plte: Vec<u8> = palette.iter().flat_map(|c| [*c as u8, (*c >> 8) as u8, (*c >> 16) as u8]).collect();
            chunk(&mut png, b"PLTE", &plte);
            // The alpha of each palette entry up to the last translucent one (sorted by alpha, the
            // opaque ones come last and can be left out).
            let alpha: Vec<u8> = palette.iter().map(|c| (*c >> 24) as u8).take_while(|a| *a < 255).collect();
            if !alpha.is_empty() {
                chunk(&mut png, b"tRNS", &alpha);
            }
            palette_scanlines(&indices, w, depth)
        }
        None => {
            // 8 bits per channel, RGBA.
            ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
            chunk(&mut png, b"IHDR", &ihdr);
            let stride = w * 4;
            let mut raw = Vec::with_capacity((stride + 1) * h);
            let (mut prev, mut row) = (vec![0u8; stride], Vec::with_capacity(stride));
            for line in pixels.chunks_exact(w) {
                row.clear();
                row.extend(line.iter().flat_map(|p| p.to_le_bytes()));
                filtered_row(&mut raw, &row, &prev, 4);
                std::mem::swap(&mut prev, &mut row);
            }
            raw
        }
    };
    // sRGB, perceptual intent: the colours are meant as they're written.
    chunk(&mut png, b"sRGB", &[0]);
    chunk(&mut png, b"IDAT", &zlib(&raw));
    chunk(&mut png, b"IEND", &[]);
    Some(png)
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    let mut s = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    let sym = |n: u32, shift: u32| BASE64[((n >> shift) & 63) as usize];
    let (groups, rest) = bytes.as_chunks::<3>();
    for &[a, b, c] in groups {
        let n = (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c);
        s.extend_from_slice(&[sym(n, 18), sym(n, 12), sym(n, 6), sym(n, 0)]);
    }
    match *rest {
        [a] => {
            let n = u32::from(a) << 16;
            s.extend_from_slice(&[sym(n, 18), sym(n, 12), b'=', b'=']);
        }
        [a, b] => {
            let n = (u32::from(a) << 16) | (u32::from(b) << 8);
            s.extend_from_slice(&[sym(n, 18), sym(n, 12), sym(n, 6), b'=']);
        }
        _ => {}
    }
    // Only ASCII went in.
    String::from_utf8(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::hash;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")];
        for (plain, coded) in cases {
            assert_eq!(base64(plain.as_bytes()), coded);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn checksums_match_their_check_values() {
        assert_eq!(crc32(&[b"123456789"]), 0xcbf4_3926);
        assert_eq!(crc32(&[b"1234", b"56789"]), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(adler32(&[]), 1);
        let big = vec![0xffu8; 100_000];
        let (mut a, mut b) = (1u64, 0u64);
        for x in &big {
            a = (a + u64::from(*x)) % 65521;
            b = (b + a) % 65521;
        }
        assert_eq!(adler32(&big), ((b << 16) | a) as u32);
    }

    #[test]
    fn distance_and_length_codes() {
        for d in 1..=32768usize {
            let c = dist_code(d);
            assert!(c < 30, "{d}");
            assert!(DIST_BASE[c] as usize <= d && d < DIST_BASE[c] as usize + (1 << DIST_EXTRA[c]), "{d} → {c}");
        }
        for len in 3..=258usize {
            let c = LENGTH_CODE[len - 3] as usize;
            let top = if c == 28 { 258 } else { LENGTH_BASE[c] as usize + (1 << LENGTH_EXTRA[c]) - 1 };
            assert!(LENGTH_BASE[c] as usize <= len && len <= top, "{len} → {c}");
        }
        assert_eq!(LENGTH_CODE[255], 28);
    }

    #[test]
    fn zlib_streams_inflate_to_what_went_in() {
        let mut cases: Vec<Vec<u8>> =
            vec![vec![], vec![7], b"abc".to_vec(), b"abcd".to_vec(), vec![0; 100_000], b"hello hello hello hello!".to_vec()];
        cases.push((0..70_000u32).map(|i| (hash(i as i32, 0, 1) & 0xff) as u8).collect());
        cases.push((0..70_000u32).map(|i| (i / 300) as u8).collect());
        cases.push((0..50_000u32).map(|i| if hash(i as i32 / 7, 1, 2) & 1 == 0 { 0 } else { (i % 251) as u8 }).collect());
        for data in cases {
            let z = zlib(&data);
            let back = miniz_oxide::inflate::decompress_to_vec_zlib(&z).unwrap();
            assert_eq!(back, data, "{} bytes", data.len());
        }
        let runs = zlib(&[0; 100_000]);
        assert!(runs.len() < 1000, "runs compress: {}", runs.len());
    }

    fn decode(file: Vec<u8>) -> (png::OutputInfo, Vec<u8>, png::ColorType) {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(file));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
        let mut reader = decoder.read_info().unwrap();
        let kind = reader.info().color_type;
        let mut out = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).unwrap();
        out.truncate(info.buffer_size());
        (info, out, kind)
    }

    /// An image of `colours` colours (picked per pixel by a hash) round-trips exactly; the colour
    /// type it was written with.
    fn round_trip(w: u32, h: u32, colours: u32) -> png::ColorType {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let n = hash(x as i32 / 3, y as i32, 5) % colours;
                rgba.extend_from_slice(&[
                    (n * 37) as u8,
                    (n * 11 + x / 50) as u8,
                    (n * 101) as u8,
                    if n.is_multiple_of(3) { 0 } else { 255 - (n % 7) as u8 * 30 },
                ]);
            }
        }
        let pixels: Vec<u32> = rgba.chunks(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]])).collect();
        let (info, out, kind) = decode(encode(w, h, &pixels).unwrap());
        assert_eq!((info.width, info.height, info.color_type, info.bit_depth), (w, h, png::ColorType::Rgba, png::BitDepth::Eight));
        assert_eq!(out, rgba, "{w}×{h}, {colours} colours");
        kind
    }

    #[test]
    fn encodes_pngs_that_decode_to_the_same_pixels() {
        for (w, h) in [(1, 1), (37, 23), (64, 64), (5, 300)] {
            for colours in [1, 2, 3, 4, 9, 16, 40, 200, 100_000] {
                round_trip(w, h, colours);
            }
        }
        assert_eq!(round_trip(64, 64, 2), png::ColorType::Indexed);
        assert_eq!(round_trip(300, 64, 100_000), png::ColorType::Rgba);
    }

    #[test]
    fn palettes_are_sorted_by_alpha_and_cover_every_pixel() {
        let px = [0x8000_00ffu32, 0xff00_ff00, 0x0000_0000, 0x8000_00ff, 0x4011_2233];
        let (palette, idx) = palettize(&px).unwrap();
        assert_eq!(palette, vec![0x0000_0000, 0x4011_2233, 0x8000_00ff, 0xff00_ff00]);
        assert_eq!(idx.iter().map(|i| palette[*i as usize]).collect::<Vec<_>>(), px);
        let many: Vec<u32> = (0..257).collect();
        assert!(palettize(&many).is_none());
        assert!(palettize(&many[..256]).is_some());
    }

    #[test]
    fn refuses_mismatched_buffers() {
        assert!(encode(2, 2, &[0; 3]).is_none());
        assert!(encode(0, 0, &[]).is_none());
        assert!(encode(1, 1, &[0]).is_some());
    }
}
