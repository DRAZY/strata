//! Reading plugin packages from `.zip` files: a small reader for the two
//! methods zip tools write (stored and deflate, through `flate2`, already in
//! the build), strict about what it accepts. A plugin zip comes from
//! someone else, so every entry is checked before anything is written:
//! no absolute paths, no `..`, no links, no names Windows can't hold, no
//! duplicates, nothing encrypted, and sizes are counted as the data is
//! inflated rather than taken from the headers (a "zip bomb" stops at the
//! limit). Zip64 archives are refused; plugins are far below its sizes.

use super::package::{MAX_DEPTH, MAX_FILES, MAX_FILE_BYTES, MAX_TOTAL_BYTES, relative_parts};
use std::io::Read;

/// Bigger archives aren't read at all.
pub const MAX_ARCHIVE_BYTES: u64 = 64 << 20;

const EOCD_SIG: u32 = 0x0605_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

/// One file from the archive: its path parts (checked) and its bytes.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub parts: Vec<String>,
    pub data: Vec<u8>,
    /// The owner may run it (from the Unix mode, when the archive has one).
    pub executable: bool,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Every file in `zip` (folders are implied by the paths). Err with a
/// plain reason for anything unsafe or unsupported; nothing is half read.
pub fn read(zip: &[u8]) -> Result<Vec<Entry>, String> {
    if zip.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(format!("the zip file is larger than {} MB", MAX_ARCHIVE_BYTES >> 20));
    }
    let bad = || "it isn't a zip file, or it's damaged".to_owned();
    // The end record is in the last 22 bytes plus up to 64 KB of comment.
    let floor = zip.len().saturating_sub(22 + 0xFFFF);
    let eocd = (floor..zip.len().saturating_sub(21)).rev().find(|&i| u32_at(zip, i) == Some(EOCD_SIG)).ok_or_else(bad)?;
    let disk = u16_at(zip, eocd + 4).ok_or_else(bad)?;
    let cd_disk = u16_at(zip, eocd + 6).ok_or_else(bad)?;
    let count = u16_at(zip, eocd + 10).ok_or_else(bad)? as usize;
    let cd_size = u32_at(zip, eocd + 12).ok_or_else(bad)? as usize;
    let cd_start = u32_at(zip, eocd + 16).ok_or_else(bad)? as usize;
    if disk != 0 || cd_disk != 0 {
        return Err("split (multi-part) zip files aren't supported".into());
    }
    if count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_start == 0xFFFF_FFFF {
        return Err("Zip64 archives aren't supported".into());
    }
    if count > MAX_FILES {
        return Err(format!("the zip holds more than {MAX_FILES} entries"));
    }
    if cd_start.checked_add(cd_size).is_none_or(|end| end > eocd) {
        return Err(bad());
    }
    let mut out: Vec<Entry> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut total: u64 = 0;
    let mut at = cd_start;
    for _ in 0..count {
        if u32_at(zip, at) != Some(CENTRAL_SIG) {
            return Err(bad());
        }
        let made_by = u16_at(zip, at + 4).ok_or_else(bad)?;
        let flags = u16_at(zip, at + 8).ok_or_else(bad)?;
        let method = u16_at(zip, at + 10).ok_or_else(bad)?;
        let crc = u32_at(zip, at + 16).ok_or_else(bad)?;
        let csize = u32_at(zip, at + 20).ok_or_else(bad)? as u64;
        let usize_ = u32_at(zip, at + 24).ok_or_else(bad)? as u64;
        let name_len = u16_at(zip, at + 28).ok_or_else(bad)? as usize;
        let extra_len = u16_at(zip, at + 30).ok_or_else(bad)? as usize;
        let comment_len = u16_at(zip, at + 32).ok_or_else(bad)? as usize;
        let external = u32_at(zip, at + 38).ok_or_else(bad)?;
        let local = u32_at(zip, at + 42).ok_or_else(bad)? as usize;
        let raw_name = zip.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        at += 46 + name_len + extra_len + comment_len;

        let name = std::str::from_utf8(raw_name).map_err(|_| "an entry's name isn't valid UTF-8".to_owned())?;
        let shown: String = name.chars().take(80).collect();
        if csize == 0xFFFF_FFFF || usize_ == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err("Zip64 archives aren't supported".into());
        }
        if flags & 1 != 0 {
            return Err(format!("\"{shown}\" is encrypted"));
        }
        // Unix-made archives keep the file type in the top bits of the attributes.
        let unix_mode = (made_by >> 8 == 3).then_some(external >> 16);
        let file_type = unix_mode.map(|m| m & 0o170_000);
        if file_type == Some(0o120_000) {
            return Err(format!("\"{shown}\" is a symbolic link; plugin packages can't contain links"));
        }
        if file_type.is_some_and(|t| t != 0 && t != 0o100_000 && t != 0o040_000) {
            return Err(format!("\"{shown}\" isn't a plain file or folder"));
        }
        if name.contains('\\') {
            return Err(format!("\"{shown}\" uses backslashes in its path"));
        }
        let is_dir = name.ends_with('/') || file_type == Some(0o040_000);
        let path = name.trim_end_matches('/');
        let odd = path.split('/').find(|p| !p.is_empty() && *p != "." && *p != ".." && !portable_name(p));
        if let Some(p) = odd {
            return Err(format!("\"{shown}\" has a name (\"{p}\") that can't be used on every system"));
        }
        let parts = relative_parts(path).ok_or_else(|| format!("\"{shown}\" points outside the package (an absolute path or \"..\")"))?;
        // As deep as an installed package may be (see `package::walk`).
        if parts.len() > MAX_DEPTH + 1 {
            return Err(format!("\"{shown}\" is nested too deeply"));
        }
        let key = parts.join("/").to_lowercase();
        if is_dir {
            if usize_ != 0 {
                return Err(format!("\"{shown}\" is a folder with data"));
            }
            continue;
        }
        // Two names one case-insensitive file system would make one file.
        if seen.contains(&key) {
            return Err(format!("\"{shown}\" is in the zip twice"));
        }
        seen.push(key);
        if usize_ > MAX_FILE_BYTES {
            return Err(format!("\"{shown}\" is larger than {} MB", MAX_FILE_BYTES >> 20));
        }
        if total + usize_ > MAX_TOTAL_BYTES {
            return Err(format!("the files in the zip add up to more than {} MB", MAX_TOTAL_BYTES >> 20));
        }
        // The local header repeats the name; its own lengths say where the data starts.
        if u32_at(zip, local) != Some(LOCAL_SIG) {
            return Err(bad());
        }
        let lname = u16_at(zip, local + 26).ok_or_else(bad)? as usize;
        let lextra = u16_at(zip, local + 28).ok_or_else(bad)? as usize;
        if zip.get(local + 30..local + 30 + lname) != Some(raw_name) {
            return Err(format!("\"{shown}\" has two different names in the zip"));
        }
        let start = local + 30 + lname + lextra;
        let body = zip.get(start..start.saturating_add(csize as usize)).ok_or_else(bad)?;
        // Inflated at most to the size it claims (plus one byte, to notice
        // more), which is within the limits checked above.
        let budget = usize_ + 1;
        let data = match method {
            0 => body.to_vec(),
            8 => {
                let mut data = Vec::new();
                flate2::read::DeflateDecoder::new(body).take(budget).read_to_end(&mut data).map_err(|e| format!("\"{shown}\" couldn't be unpacked: {e}"))?;
                data
            }
            m => return Err(format!("\"{shown}\" uses compression method {m}; only stored and deflate are supported")),
        };
        if data.len() as u64 != usize_ {
            return Err(format!("\"{shown}\" isn't the size the zip says (damaged, or made to unpack bigger)"));
        }
        let mut check = flate2::Crc::new();
        check.update(&data);
        if check.sum() != crc {
            return Err(format!("\"{shown}\" is damaged (its checksum doesn't match)"));
        }
        total += data.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(format!("the files in the zip add up to more than {} MB", MAX_TOTAL_BYTES >> 20));
        }
        out.push(Entry { parts, data, executable: unix_mode.is_some_and(|m| m & 0o100 != 0) });
    }
    Ok(out)
}

/// A file or folder name every system Strata runs on can hold.
pub fn portable_name(name: &str) -> bool {
    const RESERVED: &[&str] = &["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"];
    let stem = name.split('.').next().unwrap_or("").to_lowercase();
    !name.is_empty()
        && name.len() <= 255
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        && !RESERVED.contains(&stem.as_str())
}

/// Build a zip in memory (tests: archives made the way tools make them, and
/// ones made to attack the reader).
#[cfg(test)]
pub mod build {
    use std::io::Write;

    pub struct Item<'a> {
        pub name: &'a str,
        pub data: &'a [u8],
        pub deflate: bool,
        /// Unix mode (with the file type bits); None: made on another system.
        pub mode: Option<u32>,
        /// Lie about the uncompressed size in the headers.
        pub claimed_size: Option<u32>,
    }

    impl<'a> Item<'a> {
        pub fn file(name: &'a str, data: &'a [u8]) -> Self {
            Item { name, data, deflate: true, mode: Some(0o100_644), claimed_size: None }
        }
    }

    pub fn zip(items: &[Item]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for it in items {
            let mut crc = flate2::Crc::new();
            crc.update(it.data);
            let body = if it.deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(it.data).unwrap();
                e.finish().unwrap()
            } else {
                it.data.to_vec()
            };
            let usize_ = it.claimed_size.unwrap_or(it.data.len() as u32);
            let method: u16 = if it.deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend(0x0403_4b50u32.to_le_bytes());
            out.extend(20u16.to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(method.to_le_bytes());
            out.extend([0u8; 4]);
            out.extend(crc.sum().to_le_bytes());
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(usize_.to_le_bytes());
            out.extend((it.name.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(it.name.as_bytes());
            out.extend(&body);
            central.extend(0x0201_4b50u32.to_le_bytes());
            let made_by: u16 = if it.mode.is_some() { 3 << 8 | 20 } else { 20 };
            central.extend(made_by.to_le_bytes());
            central.extend(20u16.to_le_bytes());
            central.extend(0u16.to_le_bytes());
            central.extend(method.to_le_bytes());
            central.extend([0u8; 4]);
            central.extend(crc.sum().to_le_bytes());
            central.extend((body.len() as u32).to_le_bytes());
            central.extend(usize_.to_le_bytes());
            central.extend((it.name.len() as u16).to_le_bytes());
            central.extend([0u8; 8]);
            central.extend((it.mode.unwrap_or(0) << 16).to_le_bytes());
            central.extend(offset.to_le_bytes());
            central.extend(it.name.as_bytes());
        }
        let cd_start = out.len() as u32;
        out.extend(&central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((items.len() as u16).to_le_bytes());
        out.extend((items.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(cd_start.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::build::{Item, zip};
    use super::*;

    #[test]
    fn reads_stored_and_deflated_files_with_folders() {
        let z = zip(&[
            Item { name: "pkg/", data: b"", deflate: false, mode: Some(0o040_755), claimed_size: None },
            Item::file("pkg/strata-plugin.json", b"{}"),
            Item { name: "pkg/bin/run.sh", data: b"#!/bin/sh\necho hi\n", deflate: false, mode: Some(0o100_755), claimed_size: None },
            Item { name: "pkg/README", data: &[b'x'; 5000], deflate: true, mode: None, claimed_size: None },
        ]);
        let entries = read(&z).unwrap();
        let names: Vec<String> = entries.iter().map(|e| e.parts.join("/")).collect();
        assert_eq!(names, ["pkg/strata-plugin.json", "pkg/bin/run.sh", "pkg/README"]);
        assert_eq!(entries[1].data, b"#!/bin/sh\necho hi\n");
        assert!(entries[1].executable && !entries[0].executable);
        assert_eq!(entries[2].data.len(), 5000);
    }

    #[test]
    fn zip_slip_and_absolute_paths_are_refused() {
        for name in ["../evil.sh", "pkg/../../evil.sh", "/etc/passwd", "pkg/./x", "C:/Windows/x.dll", "a//b", ""] {
            let e = read(&zip(&[Item::file(name, b"x")])).unwrap_err();
            assert!(e.contains("outside the package") || e.contains("can't be used"), "{name}: {e}");
        }
        let e = read(&zip(&[Item::file("pkg\\..\\evil", b"x")])).unwrap_err();
        assert!(e.contains("backslashes"), "{e}");
        for name in ["pkg/con.txt", "pkg/a:b", "pkg/trailing.", "pkg/x\u{1}"] {
            let e = read(&zip(&[Item::file(name, b"x")])).unwrap_err();
            assert!(e.contains("can't be used on every system"), "{name}: {e}");
        }
    }

    #[test]
    fn deep_nesting_is_refused() {
        let deep = format!("{}x", "d/".repeat(MAX_DEPTH + 1));
        assert!(read(&zip(&[Item::file(&deep, b"x")])).unwrap_err().contains("nested too deeply"));
        let ok = format!("{}x", "d/".repeat(MAX_DEPTH));
        assert!(read(&zip(&[Item::file(&ok, b"x")])).is_ok());
    }

    #[test]
    fn links_and_special_files_are_refused() {
        let link = Item { name: "pkg/link", data: b"/etc/passwd", deflate: false, mode: Some(0o120_777), claimed_size: None };
        assert!(read(&zip(&[link])).unwrap_err().contains("symbolic link"));
        let fifo = Item { name: "pkg/fifo", data: b"", deflate: false, mode: Some(0o010_644), claimed_size: None };
        assert!(read(&zip(&[fifo])).unwrap_err().contains("isn't a plain file"));
    }

    #[test]
    fn duplicates_even_by_case_are_refused() {
        let e = read(&zip(&[Item::file("pkg/Run.sh", b"a"), Item::file("pkg/run.sh", b"b")])).unwrap_err();
        assert!(e.contains("twice"), "{e}");
    }

    #[test]
    fn sizes_are_counted_while_unpacking() {
        // A small archive whose one file inflates far past what it claims.
        let bomb = vec![0u8; 4 << 20];
        let lie = Item { name: "pkg/bomb", data: &bomb, deflate: true, mode: Some(0o100_644), claimed_size: Some(1000) };
        let z = zip(&[lie]);
        assert!(z.len() < 64 << 10, "{} bytes", z.len());
        let e = read(&z).unwrap_err();
        assert!(e.contains("isn't the size the zip says"), "{e}");
        // An honest file over the per-file limit is refused before inflating.
        let big = vec![0u8; (MAX_FILE_BYTES + 1) as usize];
        let e = read(&zip(&[Item::file("pkg/big", &big)])).unwrap_err();
        assert!(e.contains("larger than"), "{e}");
        // Many files adding up past the total.
        let chunk = vec![7u8; (MAX_FILE_BYTES - 1) as usize];
        let names: Vec<String> = (0..(MAX_TOTAL_BYTES / MAX_FILE_BYTES + 2)).map(|i| format!("pkg/f{i}")).collect();
        let items: Vec<Item> = names.iter().map(|n| Item::file(n, &chunk)).collect();
        let e = read(&zip(&items)).unwrap_err();
        assert!(e.contains("add up to more than"), "{e}");
    }

    #[test]
    fn damage_encryption_and_odd_methods() {
        let mut z = zip(&[Item { name: "pkg/a", data: b"hello", deflate: false, mode: None, claimed_size: None }]);
        // Flip a byte of the stored data: the checksum catches it.
        let at = z.windows(5).position(|w| w == b"hello").unwrap();
        z[at] = b'j';
        assert!(read(&z).unwrap_err().contains("checksum"));
        // Encrypted flag in the central directory.
        let mut z = zip(&[Item::file("pkg/a", b"x")]);
        let cd = z.windows(4).position(|w| w == 0x0201_4b50u32.to_le_bytes()).unwrap();
        z[cd + 8] |= 1;
        assert!(read(&z).unwrap_err().contains("encrypted"));
        // An unknown method.
        let mut z = zip(&[Item { name: "pkg/a", data: b"x", deflate: false, mode: None, claimed_size: None }]);
        let cd = z.windows(4).position(|w| w == 0x0201_4b50u32.to_le_bytes()).unwrap();
        z[cd + 10] = 14;
        assert!(read(&z).unwrap_err().contains("compression method 14"));
        assert!(read(b"not a zip at all").unwrap_err().contains("isn't a zip"));
        assert!(read(&z[..z.len() - 3]).is_err());
    }
}
