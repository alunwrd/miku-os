// QOI: the one image format decoded at runtime
//
// A wallpaper chosen in a config is read from a file at runtime. No Huffman
// tables, sliding window, or allocation; one pass, with each chunk decided by
// the top two bits of its first byte. Lossless and made by
// `magick x.jpg x.qoi`.
//
// The build converts anything in `assets/wallpapers/` that `image` reads into
// this, already scaled.
//
// Output is BGRA, the framebuffer's byte order and what `shm` buffers hold. Not
// premultiplied; the alpha channel is carried straight through.
//
// The format: https://qoiformat.org/qoi-specification.pdf

/// The fourteen-byte header: `qoif`, width, height, channels, colorspace
pub const HEADER: usize = 14;

/// The eight bytes ending a stream: seven zeros and a one
const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];

const OP_INDEX: u8 = 0x00;
const OP_DIFF:  u8 = 0x40;
const OP_LUMA:  u8 = 0x80;
const OP_RUN:   u8 = 0xC0;
const OP_RGB:   u8 = 0xFE;
const OP_RGBA:  u8 = 0xFF;
const MASK2:    u8 = 0xC0;

/// What a file says it holds, without decoding. A file that is not a QOI gets
/// `None`
pub fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < HEADER || &bytes[..4] != b"qoif" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let h = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    // Channels must be 3 or 4
    if w == 0 || h == 0 || !matches!(bytes[12], 3 | 4) {
        return None;
    }
    Some((w, h))
}

/// Decodes into `out`, which must hold `w * h * 4` bytes of BGRA. Returns the
/// rows filled; a short answer means the file ended early, and a caller can use
/// what did arrive
pub fn decode(bytes: &[u8], out: &mut [u8]) -> usize {
    let Some(mut d) = Decoder::new(bytes) else { return 0 };
    if out.len() < d.w * d.h * 4 {
        return 0;
    }
    d.rows(bytes, out, usize::MAX)
}

/// The same decode, a few rows at a time. The compositor decodes the wallpaper
/// between frames, a millisecond's worth each, since a whole 1920x1080 one took
/// 15 ms. The stream is read once, front to back; `decode` is this with no
/// limit
pub struct Decoder {
    w: usize,
    h: usize,
    // The format's state: the previous pixel, and 64 earlier pixels indexed by
    // a hash of themselves
    px: [u8; 4],              // r, g, b, a
    index: [[u8; 4]; 64],
    run: usize,
    /// Where the next chunk starts in the stream
    p: usize,
    /// Pixels written so far
    written: usize,
    /// The stream ran out before the picture did
    short: bool,
}

impl Decoder {
    /// Ready to decode `bytes`, or `None` if it is not a QOI
    pub fn new(bytes: &[u8]) -> Option<Self> {
        let (w, h) = size(bytes)?;
        Some(Self {
            w: w as usize,
            h: h as usize,
            px: [0, 0, 0, 255],
            index: [[0; 4]; 64],
            run: 0,
            p: HEADER,
            written: 0,
            short: false,
        })
    }

    /// Whether nothing is left: every row, or the stream ended
    pub fn done(&self) -> bool {
        self.short || self.written >= self.w * self.h
    }

    /// Whether every pixel of the picture has been decoded
    pub fn complete(&self) -> bool {
        self.written >= self.w * self.h
    }

    /// The picture's size, in pixels
    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// Decodes up to `more` rows of `bytes` into `out`, the whole picture's
    /// `w * h * 4` bytes of BGRA. Returns rows filled in all, below the height
    /// only if the file was short
    pub fn rows(&mut self, bytes: &[u8], out: &mut [u8], more: usize) -> usize {
        let (w, h) = (self.w, self.h);
        if out.len() < w * h * 4 {
            self.short = true;
            return self.written / w;
        }
        let target = (self.written / w).saturating_add(more).min(h) * w;
        let end = bytes.len().saturating_sub(END.len());
        let (mut px, mut run, mut p) = (self.px, self.run, self.p);

        while self.written < target {
            if run > 0 {
                run -= 1;
            } else if p < end {
                let b1 = bytes[p];
                p += 1;
                if b1 == OP_RGB {
                    if p + 3 > end { self.short = true; break }
                    px[0] = bytes[p];
                    px[1] = bytes[p + 1];
                    px[2] = bytes[p + 2];
                    p += 3;
                } else if b1 == OP_RGBA {
                    if p + 4 > end { self.short = true; break }
                    px.copy_from_slice(&bytes[p..p + 4]);
                    p += 4;
                } else {
                    match b1 & MASK2 {
                        OP_INDEX => px = self.index[(b1 & 0x3F) as usize],
                        OP_DIFF => {
                            // Three two-bit differences, biased by two,
                            // wrapping: from 255 to 0 is a difference of one
                            px[0] = px[0].wrapping_add((b1 >> 4) & 0x03).wrapping_sub(2);
                            px[1] = px[1].wrapping_add((b1 >> 2) & 0x03).wrapping_sub(2);
                            px[2] = px[2].wrapping_add(b1 & 0x03).wrapping_sub(2);
                        }
                        OP_LUMA => {
                            if p >= end { self.short = true; break }
                            let b2 = bytes[p];
                            p += 1;
                            // Green carries the luminance; the other two are
                            // relative to it
                            let dg = (b1 & 0x3F).wrapping_sub(32);
                            px[0] = px[0].wrapping_add(dg)
                                         .wrapping_add((b2 >> 4) & 0x0F).wrapping_sub(8);
                            px[1] = px[1].wrapping_add(dg);
                            px[2] = px[2].wrapping_add(dg)
                                         .wrapping_add(b2 & 0x0F).wrapping_sub(8);
                        }
                        // 63 is the largest run the tag holds, since 0xFE and
                        // 0xFF are taken
                        _ => run = (b1 & 0x3F) as usize,
                    }
                }
                self.index[hash(px)] = px;
            } else {
                self.short = true;
                break;
            }

            let o = self.written * 4;
            out[o] = px[2];      // blue
            out[o + 1] = px[1];  // green
            out[o + 2] = px[0];  // red
            out[o + 3] = px[3];
            self.written += 1;
        }

        self.px = px;
        self.run = run;
        self.p = p;
        self.written / w
    }
}

/// Where a pixel remembers itself
fn hash(px: [u8; 4]) -> usize {
    (px[0] as usize * 3 + px[1] as usize * 5 + px[2] as usize * 7 + px[3] as usize * 11) % 64
}
