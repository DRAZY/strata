//! Files that aren't text: images open in a preview, anything else in a
//! read-only hex view. Neither can be saved (see `Document::binary`).
//!
//! Detection looks at the start of the file: an image's signature, or bytes
//! text doesn't have (a NUL, unless it's UTF-16; lots of control codes).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Bytes read to decide what a file is.
pub const SNIFF_BYTES: usize = 8192;
/// Images bigger than this aren't decoded (they open in the hex view).
const MAX_IMAGE_BYTES: u64 = 256 << 20;
/// Bytes per hex view row.
pub const ROW: usize = 16;

/// An image format the preview shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Ico,
}

impl ImageFormat {
    pub fn name(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Gif => "GIF",
            ImageFormat::Webp => "WebP",
            ImageFormat::Bmp => "BMP",
            ImageFormat::Ico => "ICO",
        }
    }

    fn decoder(self) -> image::ImageFormat {
        match self {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            ImageFormat::Gif => image::ImageFormat::Gif,
            ImageFormat::Webp => image::ImageFormat::WebP,
            ImageFormat::Bmp => image::ImageFormat::Bmp,
            ImageFormat::Ico => image::ImageFormat::Ico,
        }
    }
}

/// What the start of a file says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image(ImageFormat),
    Binary,
}

/// The image format `head` starts with, by signature.
pub fn image_format(head: &[u8]) -> Option<ImageFormat> {
    let f = if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else if head.starts_with(b"\xFF\xD8\xFF") {
        ImageFormat::Jpeg
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        ImageFormat::Gif
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else if head.len() >= 26 && head.starts_with(b"BM") && matches!(u32::from_le_bytes([head[14], head[15], head[16], head[17]]), 12 | 40 | 52 | 56 | 64 | 108 | 124) {
        // A known info header size sets a bitmap apart from text starting "BM".
        ImageFormat::Bmp
    } else if head.len() >= 6 && head.starts_with(b"\0\0\x01\0") && head[4..6] != [0, 0] {
        ImageFormat::Ico
    } else {
        return None;
    };
    Some(f)
}

/// Text, an image, or other binary data, from the file's first bytes.
pub fn classify(head: &[u8]) -> Kind {
    if let Some(f) = image_format(head) {
        return Kind::Image(f);
    }
    if head.is_empty() || crate::encoding::detect(head).encoding.id().starts_with("utf-16") {
        return Kind::Text;
    }
    // A NUL, or many control codes (beyond tab, line breaks, form feed and escape).
    let control = head.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)).count();
    if head.contains(&0) || control * 10 > head.len() { Kind::Binary } else { Kind::Text }
}

/// The first `SNIFF_BYTES` of a file.
pub fn head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    File::open(path)?.take(SNIFF_BYTES as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// A decoded image, ready to become a texture.
pub struct Picture {
    pub format: ImageFormat,
    /// The image's own size, in pixels.
    pub width: u32,
    pub height: u32,
    /// Its pixels, shrunk to fit `max_side` if it was bigger.
    pub pixels: egui::ColorImage,
}

/// Decode an image (the first frame of an animation), shrunk so neither
/// side is over `max_side` (what the GPU takes).
pub fn decode_image(bytes: &[u8], format: ImageFormat, max_side: u32) -> Result<Picture, String> {
    let img = image::load_from_memory_with_format(bytes, format.decoder()).map_err(|e| e.to_string())?;
    let (width, height) = (img.width(), img.height());
    let img = if width > max_side || height > max_side { img.thumbnail(max_side, max_side) } else { img };
    let rgba = img.to_rgba8();
    let pixels = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    Ok(Picture { format, width, height, pixels })
}

/// One hex view row: offset, 16 bytes in hex (a gap after 8), and the
/// bytes as ASCII ('.' for the rest). `digits` is the offset's width.
pub fn hex_row(offset: u64, bytes: &[u8], digits: usize) -> String {
    let mut hex = String::with_capacity(ROW * 3 + 1);
    for k in 0..ROW {
        if k == ROW / 2 {
            hex.push(' ');
        }
        match bytes.get(k) {
            Some(b) => hex.push_str(&format!("{b:02x} ")),
            None => hex.push_str("   "),
        }
    }
    let ascii: String = bytes.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
    format!("{offset:0digits$x}  {hex} {ascii}")
}

/// Hex digits the offsets of a `len`-byte file need (at least 8).
pub fn offset_digits(len: u64) -> usize {
    let bits = 64 - len.saturating_sub(1).leading_zeros() as usize;
    bits.div_ceil(4).max(8)
}

/// "1.2 MB (1,234,567 bytes)".
pub fn size_label(len: u64) -> String {
    let human = match len {
        n if n < 1024 => return format!("{n} byte{}", if n == 1 { "" } else { "s" }),
        n if n < 1 << 20 => format!("{:.1} KB", n as f64 / 1024.0),
        n if n < 1 << 30 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n => format!("{:.1} GB", n as f64 / (1u64 << 30) as f64),
    };
    let digits = len.to_string();
    let mut grouped = String::new();
    for (k, c) in digits.chars().enumerate() {
        if k > 0 && (digits.len() - k).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{human} ({grouped} bytes)")
}

/// Reads the part of a file the hex view shows, a window at a time, so a
/// file of any size opens at once.
pub struct HexReader {
    path: PathBuf,
    /// (offset, bytes) last read.
    window: (u64, Vec<u8>),
}

impl HexReader {
    pub fn new(path: PathBuf) -> Self {
        Self { path, window: (0, Vec::new()) }
    }

    /// Up to `len` bytes at `offset` (fewer at the end of the file, none if
    /// it can't be read).
    pub fn read(&mut self, offset: u64, len: usize) -> &[u8] {
        let (start, bytes) = &self.window;
        let have = offset >= *start && offset + len as u64 <= start + bytes.len() as u64;
        if !have {
            // Read a little extra on both sides so scrolling rarely reads.
            let from = offset.saturating_sub(64 * ROW as u64);
            let want = len + 128 * ROW;
            let mut buf = Vec::with_capacity(want);
            let read = File::open(&self.path).and_then(|mut f| {
                f.seek(SeekFrom::Start(from))?;
                f.take(want as u64).read_to_end(&mut buf)
            });
            if read.is_err() {
                buf.clear();
            }
            self.window = (from, buf);
        }
        let (start, bytes) = &self.window;
        let at = (offset.saturating_sub(*start) as usize).min(bytes.len());
        &bytes[at..(at + len).min(bytes.len())]
    }
}

/// A tab showing a file that isn't text.
pub struct BinaryView {
    pub len: u64,
    pub content: Content,
}

pub enum Content {
    /// The pixels go to the GPU on the first draw.
    /// `actual_size`: 100% (scrolling) rather than shrunk to fit; `scale`:
    /// screen pixels per image pixel, as last drawn.
    Image { picture: Picture, texture: Option<egui::TextureHandle>, actual_size: bool, scale: f32 },
    Hex(HexReader),
}

impl BinaryView {
    /// The view for `path`, or None if it's text. `max_side`: the biggest
    /// image side the GPU takes. An image that won't decode shows as hex.
    pub fn open(path: &Path, max_side: u32) -> io::Result<Option<BinaryView>> {
        let head = head(path)?;
        let len = std::fs::metadata(path)?.len();
        let content = match classify(&head) {
            Kind::Text => return Ok(None),
            Kind::Image(format) if len <= MAX_IMAGE_BYTES => match decode_image(&std::fs::read(path)?, format, max_side) {
                Ok(picture) => Content::Image { picture, texture: None, actual_size: false, scale: 1.0 },
                Err(e) => {
                    log::info!("{} looks like {} but doesn't decode ({e}): showing it as hex", path.display(), format.name());
                    Content::Hex(HexReader::new(path.to_owned()))
                }
            },
            _ => Content::Hex(HexReader::new(path.to_owned())),
        };
        Ok(Some(BinaryView { len, content }))
    }

    /// For the status bar: "PNG image · 640 × 480 · 12.3 KB (12,345 bytes)".
    pub fn describe(&self) -> String {
        match &self.content {
            Content::Image { picture: p, scale, .. } => {
                format!("{} image · {} × {} · {} · {:.0}%", p.format.name(), p.width, p.height, size_label(self.len), scale * 100.0)
            }
            Content::Hex(_) => format!("Binary · {}", size_label(self.len)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_and_binary_bytes() {
        assert_eq!(classify(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Kind::Image(ImageFormat::Png));
        assert_eq!(classify(b"\xFF\xD8\xFF\xE0\0\x10JFIF"), Kind::Image(ImageFormat::Jpeg));
        assert_eq!(classify(b"GIF89a\x01\0\x01\0"), Kind::Image(ImageFormat::Gif));
        assert_eq!(classify(b"RIFF\x24\0\0\0WEBPVP8 "), Kind::Image(ImageFormat::Webp));
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 8]);
        assert_eq!(classify(&bmp), Kind::Image(ImageFormat::Bmp));
        assert_eq!(classify(b"BMW owners' club minutes, page one of many"), Kind::Text, "text starting BM");
        assert_eq!(classify(b"\0\0\x01\0\x01\0\x10\x10"), Kind::Image(ImageFormat::Ico));
        assert_eq!(classify(b"\x7fELF\x02\x01\x01\0\0\0\0\0"), Kind::Binary);
        assert_eq!(classify(&[0x01, 0x02, 0x03, 0x04, b'a', b'b', 0x05, 0x06]), Kind::Binary, "control codes without a NUL");
        assert_eq!(classify(b"fn main() {\n\tprintln!(\"hi\");\n}\n"), Kind::Text);
        assert_eq!(classify("caf\u{e9} \x1b[1mbold\x1b[0m\x0c".as_bytes()), Kind::Text);
        assert_eq!(classify(b"caf\xE9 latin-1"), Kind::Text);
        assert_eq!(classify(b""), Kind::Text);
        let utf16: Vec<u8> = "plain text\r\n".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(classify(&utf16), Kind::Text, "UTF-16 has NULs but is text");
    }

    #[test]
    fn hex_rows() {
        let bytes: Vec<u8> = (0..16).map(|b| b + 0x40).collect();
        assert_eq!(hex_row(0x10, &bytes, 8), "00000010  40 41 42 43 44 45 46 47  48 49 4a 4b 4c 4d 4e 4f  @ABCDEFGHIJKLMNO");
        assert_eq!(hex_row(0x20, b"hi\n\0\xff", 8), format!("00000020  68 69 0a 00 ff{}hi...", " ".repeat(36)));
        assert_eq!(hex_row(0, b"x", 8).len(), hex_row(0, &bytes, 8).len() - 15, "short rows keep the ASCII column in place");
        assert_eq!(offset_digits(0), 8);
        assert_eq!(offset_digits(1 << 32), 8);
        assert_eq!(offset_digits((1 << 32) + 1), 9);
        assert_eq!(size_label(1), "1 byte");
        assert_eq!(size_label(999), "999 bytes");
        assert_eq!(size_label(1_234_567), "1.2 MB (1,234,567 bytes)");
        assert_eq!(size_label(2048), "2.0 KB (2,048 bytes)");
    }

    #[test]
    fn the_reader_reads_any_window() {
        let path = std::env::temp_dir().join(format!("strata-hex-{}.bin", std::process::id()));
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let mut r = HexReader::new(path.clone());
        assert_eq!(r.read(0, 32), &data[..32]);
        assert_eq!(r.read(50_000, 160), &data[50_000..50_160]);
        assert_eq!(r.read(99_990, 32), &data[99_990..], "short at the end");
        assert!(r.read(200_000, 16).is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn images_decode_and_shrink_to_the_gpu_limit() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_fn(40, 20, |x, _| image::Rgba([x as u8 * 6, 0, 0, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        assert_eq!(classify(&png), Kind::Image(ImageFormat::Png));
        let p = decode_image(&png, ImageFormat::Png, 4096).unwrap();
        assert_eq!((p.width, p.height, p.pixels.size), (40, 20, [40, 20]));
        let small = decode_image(&png, ImageFormat::Png, 10).unwrap();
        assert_eq!((small.width, small.height), (40, 20), "the real size is kept for the status bar");
        assert_eq!(small.pixels.size, [10, 5]);
        assert!(decode_image(b"\x89PNG\r\n\x1a\nbroken", ImageFormat::Png, 4096).is_err());
    }
}
