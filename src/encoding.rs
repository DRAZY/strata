//! Text file encodings: detection on open, decoding, and encoding on save.
//!
//! Legacy encodings go through `encoding_rs`. UTF-8, UTF-16 and true
//! ISO-8859-1 (which the web standard folds into Windows-1252) are handled
//! here directly.

use std::borrow::Cow;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    Utf8,
    Utf16Le,
    Utf16Be,
    /// Every byte is the code point of the same value.
    Latin1,
    Rs(&'static encoding_rs::Encoding),
}

/// A file encoding as offered in the "Reopen with Encoding" / "Save with
/// Encoding" menus. Cheap to copy and compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoding {
    id: &'static str,
    name: &'static str,
    codec: Codec,
    bom: bool,
}

const fn enc(id: &'static str, name: &'static str, codec: Codec, bom: bool) -> Encoding {
    Encoding { id, name, codec, bom }
}

const fn rs(id: &'static str, name: &'static str, e: &'static encoding_rs::Encoding) -> Encoding {
    enc(id, name, Codec::Rs(e), false)
}

use encoding_rs as e;

// The menus and session code in app.rs are the users of most of this.
impl Encoding {
    pub const UTF8:Encoding = enc("utf-8", "UTF-8", Codec::Utf8, false);
    pub const UTF8_BOM: Encoding = enc("utf-8-bom", "UTF-8 with BOM", Codec::Utf8, true);
    pub const UTF16LE: Encoding = enc("utf-16le", "UTF-16 LE", Codec::Utf16Le, false);
    pub const UTF16LE_BOM: Encoding = enc("utf-16le-bom", "UTF-16 LE with BOM", Codec::Utf16Le, true);
    pub const UTF16BE: Encoding = enc("utf-16be", "UTF-16 BE", Codec::Utf16Be, false);
    pub const UTF16BE_BOM: Encoding = enc("utf-16be-bom", "UTF-16 BE with BOM", Codec::Utf16Be, true);
    pub const WINDOWS_1252: Encoding = rs("windows-1252", "Western (Windows 1252)", &e::WINDOWS_1252_INIT);
    pub const ISO_8859_1: Encoding = enc("iso-8859-1", "Western (ISO 8859-1)", Codec::Latin1, false);

    /// Every encoding, in menu order.
    pub fn all() -> &'static [Encoding] {
        &ALL
    }

    /// Display name, e.g. "Western (Windows 1252)".
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Stable identifier for persisting (session files); see [`Encoding::from_id`].
    pub fn id(&self) -> &'static str {
        self.id
    }

    pub fn from_id(id: &str) -> Option<Encoding> {
        ALL.iter().copied().find(|e| e.id.eq_ignore_ascii_case(id))
    }


    fn bom_bytes(&self) -> &'static [u8] {
        match self.codec {
            Codec::Utf8 => b"\xEF\xBB\xBF",
            Codec::Utf16Le => b"\xFF\xFE",
            Codec::Utf16Be => b"\xFE\xFF",
            _ => b"",
        }
    }
}

impl Default for Encoding {
    fn default() -> Self {
        Encoding::UTF8
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}

static ALL:[Encoding; 40] = [
    Encoding::UTF8,
    Encoding::UTF8_BOM,
    Encoding::UTF16LE,
    Encoding::UTF16LE_BOM,
    Encoding::UTF16BE,
    Encoding::UTF16BE_BOM,
    Encoding::WINDOWS_1252,
    Encoding::ISO_8859_1,
    rs("iso-8859-15", "Western (ISO 8859-15)", &e::ISO_8859_15_INIT),
    rs("macintosh", "Western (Mac Roman)", &e::MACINTOSH_INIT),
    rs("iso-8859-3", "Western (ISO 8859-3)", &e::ISO_8859_3_INIT),
    rs("iso-8859-14", "Celtic (ISO 8859-14)", &e::ISO_8859_14_INIT),
    rs("iso-8859-10", "Nordic (ISO 8859-10)", &e::ISO_8859_10_INIT),
    rs("windows-1250", "Central European (Windows 1250)", &e::WINDOWS_1250_INIT),
    rs("iso-8859-2", "Central European (ISO 8859-2)", &e::ISO_8859_2_INIT),
    rs("iso-8859-16", "Romanian (ISO 8859-16)", &e::ISO_8859_16_INIT),
    rs("windows-1257", "Baltic (Windows 1257)", &e::WINDOWS_1257_INIT),
    rs("iso-8859-4", "Baltic (ISO 8859-4)", &e::ISO_8859_4_INIT),
    rs("iso-8859-13", "Estonian (ISO 8859-13)", &e::ISO_8859_13_INIT),
    rs("windows-1251", "Cyrillic (Windows 1251)", &e::WINDOWS_1251_INIT),
    rs("iso-8859-5", "Cyrillic (ISO 8859-5)", &e::ISO_8859_5_INIT),
    rs("koi8-r", "Cyrillic (KOI8-R)", &e::KOI8_R_INIT),
    rs("koi8-u", "Cyrillic (KOI8-U)", &e::KOI8_U_INIT),
    rs("x-mac-cyrillic", "Cyrillic (Mac)", &e::X_MAC_CYRILLIC_INIT),
    rs("windows-1253", "Greek (Windows 1253)", &e::WINDOWS_1253_INIT),
    rs("iso-8859-7", "Greek (ISO 8859-7)", &e::ISO_8859_7_INIT),
    rs("windows-1254", "Turkish (Windows 1254)", &e::WINDOWS_1254_INIT),
    rs("windows-1255", "Hebrew (Windows 1255)", &e::WINDOWS_1255_INIT),
    rs("iso-8859-8", "Hebrew (ISO 8859-8)", &e::ISO_8859_8_INIT),
    rs("windows-1256", "Arabic (Windows 1256)", &e::WINDOWS_1256_INIT),
    rs("iso-8859-6", "Arabic (ISO 8859-6)", &e::ISO_8859_6_INIT),
    rs("windows-1258", "Vietnamese (Windows 1258)", &e::WINDOWS_1258_INIT),
    rs("windows-874", "Thai (Windows 874)", &e::WINDOWS_874_INIT),
    rs("shift_jis", "Japanese (Shift_JIS)", &e::SHIFT_JIS_INIT),
    rs("euc-jp", "Japanese (EUC-JP)", &e::EUC_JP_INIT),
    rs("iso-2022-jp", "Japanese (ISO-2022-JP)", &e::ISO_2022_JP_INIT),
    rs("gbk", "Chinese Simplified (GBK)", &e::GBK_INIT),
    rs("gb18030", "Chinese Simplified (GB18030)", &e::GB18030_INIT),
    rs("big5", "Chinese Traditional (Big5)", &e::BIG5_INIT),
    rs("euc-kr", "Korean (EUC-KR)", &e::EUC_KR_INIT),
];

/// Pick an encoding for bytes read from disk and decode them.
/// Order: byte order mark, BOM-less UTF-16 (ASCII-heavy text with NULs at
/// every other byte), valid UTF-8, then a guess among the legacy encodings
/// (see [`guess_legacy`]). Returns (text, encoding, lossy).
pub fn decode_detect(bytes: &[u8]) -> (Cow<'_, str>, Encoding, bool) {
    let d = detect(bytes);
    (d.text, d.encoding, d.lossy)
}

/// What [`detect`] found.
pub struct Detected<'a> {
    pub text: Cow<'a, str>,
    pub encoding: Encoding,
    /// Malformed bytes were replaced with U+FFFD.
    pub lossy: bool,
    /// The bytes weren't Unicode, so the encoding is a guess from their
    /// statistics: Some(confident). None for a BOM, UTF-16 or UTF-8.
    pub guess: Option<bool>,
}

impl Detected<'_> {
    /// Worth telling the user: a legacy encoding other than Windows 1252,
    /// or any guess made with little to go on.
    pub fn worth_mentioning(&self) -> bool {
        self.guess.is_some_and(|confident| !confident || self.encoding != Encoding::WINDOWS_1252)
    }
}

/// [`decode_detect`], also saying whether the encoding was guessed.
pub fn detect(bytes: &[u8]) -> Detected<'_> {
    let enc = if bytes.starts_with(b"\xEF\xBB\xBF") {
        Encoding::UTF8_BOM
    } else if bytes.starts_with(b"\xFF\xFE") {
        Encoding::UTF16LE_BOM
    } else if bytes.starts_with(b"\xFE\xFF") {
        Encoding::UTF16BE_BOM
    } else if let Some(enc) = sniff_utf16(bytes) {
        enc
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        return Detected { text: Cow::Borrowed(text), encoding: Encoding::UTF8, lossy: false, guess: None };
    } else {
        let (encoding, confident) = guess_legacy(bytes);
        let (text, lossy) = decode(bytes, encoding);
        return Detected { text, encoding, lossy, guess: Some(confident) };
    };
    let (text, lossy) = decode(bytes, enc);
    Detected { text, encoding: enc, lossy, guess: None }
}

/// Fewer non-ASCII bytes than this is too little to tell legacy encodings apart.
const GUESS_MIN_NON_ASCII: usize = 16;
/// The detector looks at most this much of a file.
const GUESS_SAMPLE: usize = 1 << 20;

/// A legacy encoding for bytes that aren't UTF-8 (Shift_JIS, GBK, EUC-KR,
/// Big5, KOI8-R, Windows 1251…), guessed by `chardetng` from the bytes'
/// statistics, and whether the guess is a confident one. A guess that can't
/// decode the file cleanly, or one Strata doesn't offer, becomes Windows
/// 1252, which maps every byte so nothing is ever replaced.
pub fn guess_legacy(bytes: &[u8]) -> (Encoding, bool) {
    let sample = &bytes[..bytes.len().min(GUESS_SAMPLE)];
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(sample, sample.len() == bytes.len());
    let guessed = detector.guess(None, chardetng::Utf8Detection::Deny);
    let enough = sample.iter().filter(|b| !b.is_ascii()).count() >= GUESS_MIN_NON_ASCII;
    let ours = ALL.iter().copied().find(|e| matches!(e.codec, Codec::Rs(c) if c == guessed));
    // Bytes Windows 1252 leaves undefined say it's something else (0x81
    // leads most Shift_JIS punctuation, say) that the detector ruled out.
    let undefined_1252 = || sample.iter().any(|b| matches!(b, 0x81 | 0x8D | 0x8F | 0x90 | 0x9D));
    match ours {
        Some(enc) if enc == Encoding::WINDOWS_1252 => (enc, enough && !undefined_1252()),
        Some(enc) if guessed.decode_without_bom_handling_and_without_replacement(bytes).is_some() => (enc, enough),
        _ => (Encoding::WINDOWS_1252, false),
    }
}

/// UTF-16 without a BOM: in the first few KB, at least a fifth of the
/// two-byte units have one NUL half and almost none have the other, and the
/// whole file decodes cleanly (binary files with NULs fail that).
fn sniff_utf16(bytes: &[u8]) -> Option<Encoding> {
    let sample = &bytes[..bytes.len().min(4096) & !1];
    if sample.len() < 4 || !sample.contains(&0) {
        return None;
    }
    let units = sample.len() / 2;
    let (mut even, mut odd) = (0, 0);
    for [a, b] in sample.as_chunks::<2>().0 {
        even += (*a == 0) as usize;
        odd += (*b == 0) as usize;
    }
    let enc = if odd * 5 >= units && even * 20 <= units {
        Encoding::UTF16LE
    } else if even * 5 >= units && odd * 20 <= units {
        Encoding::UTF16BE
    } else {
        return None;
    };
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let codec = if enc == Encoding::UTF16LE { e::UTF_16LE } else { e::UTF_16BE };
    codec.decode_without_bom_handling_and_without_replacement(bytes).map(|_| enc)
}

/// Decode with a chosen encoding. A leading BOM of the same Unicode family is
/// dropped (it never belongs in the text). Returns (text, lossy) where lossy
/// means malformed bytes were replaced with U+FFFD.
pub fn decode(bytes: &[u8], enc: Encoding) -> (Cow<'_, str>, bool) {
    let bom = enc.bom_bytes();
    let bytes = if !bom.is_empty() && bytes.starts_with(bom) { &bytes[bom.len()..] } else { bytes };
    match enc.codec {
        Codec::Utf8 => match std::str::from_utf8(bytes) {
            Ok(text) => (Cow::Borrowed(text), false),
            Err(_) => (Cow::Owned(String::from_utf8_lossy(bytes).into_owned()), true),
        },
        Codec::Latin1 => (Cow::Owned(bytes.iter().map(|&b| b as char).collect()), false),
        Codec::Utf16Le => decode_rs(e::UTF_16LE, bytes),
        Codec::Utf16Be => decode_rs(e::UTF_16BE, bytes),
        Codec::Rs(codec) => decode_rs(codec, bytes),
    }
}

fn decode_rs<'a>(codec: &'static encoding_rs::Encoding, bytes: &'a [u8]) -> (Cow<'a, str>, bool) {
    codec.decode_without_bom_handling(bytes)
}

/// The first character `enc` can't represent, as (byte offset into the
/// concatenated chunks, char).
pub fn find_unencodable<'a>(chunks: impl Iterator<Item = &'a str>, enc: Encoding) -> Option<(usize, char)> {
    let mut base = 0;
    match enc.codec {
        Codec::Utf8 | Codec::Utf16Le | Codec::Utf16Be => None,
        Codec::Latin1 => {
            for chunk in chunks {
                if let Some((i, c)) = chunk.char_indices().find(|&(_, c)| c as u32 > 0xFF) {
                    return Some((base + i, c));
                }
                base += chunk.len();
            }
            None
        }
        Codec::Rs(codec) => {
            let mut encoder = codec.new_encoder();
            let mut scratch = [0u8; 8192];
            for chunk in chunks {
                let mut pos = 0;
                loop {
                    let (res, read, _) = encoder.encode_from_utf8_without_replacement(&chunk[pos..], &mut scratch, false);
                    pos += read;
                    match res {
                        encoding_rs::EncoderResult::InputEmpty => break,
                        encoding_rs::EncoderResult::OutputFull => {}
                        encoding_rs::EncoderResult::Unmappable(c) => return Some((base + pos - c.len_utf8(), c)),
                    }
                }
                base += chunk.len();
            }
            None
        }
    }
}

/// Write the chunks in `enc`, BOM first if it has one. Fails with
/// `InvalidData` on a character the encoding can't represent (call
/// [`find_unencodable`] first to avoid a half-written file).
pub fn encode_to<'a, W: Write>(chunks: impl Iterator<Item = &'a str>, enc: Encoding, out: &mut W) -> io::Result<()> {
    if enc.bom {
        out.write_all(enc.bom_bytes())?;
    }
    match enc.codec {
        Codec::Utf8 => {
            for chunk in chunks {
                out.write_all(chunk.as_bytes())?;
            }
        }
        Codec::Utf16Le | Codec::Utf16Be => {
            let le = enc.codec == Codec::Utf16Le;
            let mut buf = Vec::with_capacity(16 * 1024);
            for chunk in chunks {
                buf.clear();
                for unit in chunk.encode_utf16() {
                    buf.extend_from_slice(&if le { unit.to_le_bytes() } else { unit.to_be_bytes() });
                }
                out.write_all(&buf)?;
            }
        }
        Codec::Latin1 => {
            let mut buf = Vec::with_capacity(16 * 1024);
            for chunk in chunks {
                buf.clear();
                for c in chunk.chars() {
                    let b = u8::try_from(c as u32).map_err(|_| unencodable(c, enc))?;
                    buf.push(b);
                }
                out.write_all(&buf)?;
            }
        }
        Codec::Rs(codec) => {
            let mut encoder = codec.new_encoder();
            let mut buf = [0u8; 16 * 1024];
            let mut feed = |src: &str, last: bool, out: &mut W| -> io::Result<()> {
                let mut pos = 0;
                loop {
                    let (res, read, written) = encoder.encode_from_utf8_without_replacement(&src[pos..], &mut buf, last);
                    pos += read;
                    out.write_all(&buf[..written])?;
                    match res {
                        encoding_rs::EncoderResult::InputEmpty => return Ok(()),
                        encoding_rs::EncoderResult::OutputFull => {}
                        encoding_rs::EncoderResult::Unmappable(c) => return Err(unencodable(c, enc)),
                    }
                }
            };
            for chunk in chunks {
                feed(chunk, false, out)?;
            }
            // Stateful encodings (ISO-2022-JP) return to ASCII at the end.
            feed("", true, out)?;
        }
    }
    Ok(())
}

fn unencodable(c: char, enc: Encoding) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("'{c}' (U+{:04X}) can't be saved as {}", c as u32, enc.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(text: &str, enc: Encoding) -> Vec<u8> {
        let mut out = Vec::new();
        encode_to(std::iter::once(text), enc, &mut out).unwrap();
        out
    }

    #[test]
    fn ids_round_trip_and_are_unique() {
        for (i, a) in Encoding::all().iter().enumerate() {
            assert_eq!(Encoding::from_id(a.id()), Some(*a));
            assert!(Encoding::all()[i + 1..].iter().all(|b| b.id() != a.id() && b.name() != a.name()));
        }
        assert_eq!(Encoding::from_id("nope"), None);
    }

    #[test]
    fn every_encoding_round_trips_ascii() {
        for &enc in Encoding::all() {
            let bytes = encode("Hello, world!\r\n", enc);
            assert_eq!(decode(&bytes, enc), (Cow::Borrowed("Hello, world!\r\n"), false), "{}", enc.name());
        }
    }

    #[test]
    fn iso_8859_1_is_not_windows_1252() {
        assert_eq!(decode(b"\x80\xe9", Encoding::ISO_8859_1).0, "\u{80}é");
        assert_eq!(decode(b"\x80\xe9", Encoding::WINDOWS_1252).0, "€é");
        assert_eq!(encode("\u{80}é", Encoding::ISO_8859_1), b"\x80\xe9");
        assert!(find_unencodable(std::iter::once("a€"), Encoding::ISO_8859_1).is_some());
    }

    #[test]
    fn iso_2022_jp_returns_to_ascii() {
        let bytes = encode("a日本", Encoding::from_id("iso-2022-jp").unwrap());
        assert!(bytes.ends_with(b"\x1b(B"), "{bytes:?}");
        assert_eq!(decode(&bytes, Encoding::from_id("iso-2022-jp").unwrap()).0, "a日本");
    }

    #[test]
    fn unencodable_reports_offset_across_chunks() {
        let chunks = ["abc", "dé漢"];
        let enc = Encoding::WINDOWS_1252;
        assert_eq!(find_unencodable(chunks.iter().copied(), enc), Some((6, '漢')));
        let err = encode_to(chunks.iter().copied(), enc, &mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(find_unencodable(chunks.iter().copied(), Encoding::from_id("gb18030").unwrap()).is_none());
    }

    /// `text` in the encoding `id`, a few times over (detection wants some text).
    fn sample(id: &str, text: &str) -> Vec<u8> {
        encode(&text.repeat(3), Encoding::from_id(id).unwrap())
    }

    #[test]
    fn legacy_encodings_are_guessed() {
        let cases = [
            ("shift_jis", "日本語のテキストファイルです。これは文字コードの判定のためのテストです。\n"),
            ("euc-jp", "日本語のテキストファイルです。これは文字コードの判定のためのテストです。\n"),
            ("gbk", "这是一个简体中文的文本文件，用来测试编码检测是否正确。我们需要足够多的汉字。\n"),
            ("big5", "這是一個繁體中文的文字檔案，用來測試編碼偵測是否正確。我們需要足夠多的漢字。\n"),
            ("euc-kr", "이것은 한국어 텍스트 파일입니다. 인코딩 감지가 올바른지 확인하기 위한 테스트입니다.\n"),
            ("koi8-r", "Это текстовый файл на русском языке. Проверяем, правильно ли определяется кодировка.\n"),
            ("windows-1251", "Это текстовый файл на русском языке. Проверяем, правильно ли определяется кодировка.\n"),
            ("windows-1253", "Αυτό είναι ένα αρχείο κειμένου στα ελληνικά. Ελέγχουμε αν η κωδικοποίηση εντοπίζεται σωστά.\n"),
        ];
        for (id, text) in cases {
            let bytes = sample(id, text);
            let d = detect(&bytes);
            // KOI8-U is KOI8-R plus a few Ukrainian letters: the same Russian text.
            let found = if id == "koi8-r" { d.encoding.id().replace("koi8-u", "koi8-r") } else { d.encoding.id().to_owned() };
            assert_eq!(found, id, "{text}");
            assert_eq!(d.text, text.repeat(3));
            assert!(!d.lossy);
            assert_eq!(d.guess, Some(true), "{id}");
            assert!(d.worth_mentioning(), "{id}");
        }
    }

    #[test]
    fn western_text_stays_windows_1252_quietly() {
        let bytes = sample("windows-1252", "Café crème, naïve façade, déjà vu — résumé of the piñata señor’s “quotes”.\n");
        let d = detect(&bytes);
        assert_eq!(d.encoding, Encoding::WINDOWS_1252);
        assert_eq!(d.guess, Some(true));
        assert!(!d.worth_mentioning());
        // Two accented letters are too little to be sure of anything.
        let d = detect(b"caf\xE9 na\xEFve\n");
        assert_eq!(d.guess, Some(false));
        assert!(d.worth_mentioning(), "a guess on little evidence is mentioned");
        // Unicode isn't a guess.
        assert_eq!(detect("日本語".as_bytes()).guess, None);
        assert_eq!(detect(b"\xFF\xFEa\x00").guess, None);
    }

    #[test]
    fn a_guess_that_cant_decode_cleanly_falls_back_to_windows_1252() {
        // Shift_JIS text cut off after a lead byte.
        let mut bytes = sample("shift_jis", "日本語のテキストファイルです。これは文字コードの判定のためのテストです。\n");
        bytes.push(0x81);
        let d = detect(&bytes);
        assert_eq!(d.encoding, Encoding::WINDOWS_1252);
        assert_eq!(d.guess, Some(false));
        assert!(!d.lossy, "nothing is replaced");
    }

    #[test]
    fn binary_with_nuls_is_not_utf16() {
        // Looks like UTF-16 LE, but holds a lone surrogate (0xD800).
        let mut bytes = b"a\x00".repeat(20);
        bytes.extend_from_slice(b"\x00\xD8a\x00");
        let (_, enc, lossy) = decode_detect(&bytes);
        assert_eq!(enc, Encoding::WINDOWS_1252);
        assert!(!lossy);
        // All NULs is valid UTF-8 and not ASCII-like UTF-16 either.
        assert_eq!(decode_detect(&[0u8; 64]).1, Encoding::UTF8);
    }
}
