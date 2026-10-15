//! Signing tool for Strata's updates (see docs/UPDATES.md).
//!
//!   cargo run --release --example strata-sign -- keygen [PATH]
//!   cargo run --release --example strata-sign -- pubkey
//!   cargo run --release --example strata-sign -- manifest VERSION NOTES.md OUT.json BASE_URL KEY=FILE...
//!   cargo run --release --example strata-sign -- verify MANIFEST.json DIR
//!
//! The private key is a 32-byte ed25519 seed, stored as one line of base64.
//! It's read from `STRATA_UPDATE_KEY`: a path to the key file, or the base64
//! itself (a CI secret); without it, from `~/.strata-update-key`. It is never
//! printed. `keygen` writes a new key (never over an existing file), owner
//! read/write only, and prints the public key to put in src/update/key.rs.

#[path = "../src/update/sig.rs"]
#[allow(dead_code)]
mod sig;

use ed25519_dalek::{Signer, SigningKey};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => keygen(args.get(1).map(PathBuf::from).unwrap_or_else(default_key_path)),
        Some("pubkey") => load_key().map(|k| print_public(&k)),
        Some("manifest") if args.len() >= 6 => manifest(&args[1], Path::new(&args[2]), Path::new(&args[3]), &args[4], &args[5..]),
        Some("verify") if args.len() == 3 => verify(Path::new(&args[1]), Path::new(&args[2])),
        _ => Err("usage: strata-sign keygen [PATH] | pubkey | manifest VERSION NOTES OUT BASE_URL KEY=FILE... | verify MANIFEST DIR".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("strata-sign: {e}");
            ExitCode::FAILURE
        }
    }
}

fn default_key_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".strata-update-key")
}

fn keygen(path: PathBuf) -> Result<(), String> {
    let mut seed = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut seed)).map_err(|e| format!("couldn't read /dev/urandom: {e}"))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(&path).map_err(|e| format!("couldn't create {} (it must not exist yet): {e}", path.display()))?;
    std::io::Write::write_all(&mut f, format!("{}\n", sig::b64_encode(&seed)).as_bytes()).map_err(|e| e.to_string())?;
    eprintln!("wrote the private key to {} (mode 600). Keep it secret and backed up.", path.display());
    print_public(&SigningKey::from_bytes(&seed));
    Ok(())
}

fn print_public(key: &SigningKey) {
    let pk = key.verifying_key().to_bytes();
    println!("public key (base64): {}", sig::b64_encode(&pk));
    let bytes: Vec<String> = pk.iter().map(|b| format!("0x{b:02x}")).collect();
    println!("for src/update/key.rs:\npub const PUBLIC_KEY: [u8; 32] = [{}];", bytes.join(", "));
}

fn load_key() -> Result<SigningKey, String> {
    let text = match std::env::var("STRATA_UPDATE_KEY") {
        Ok(v) if Path::new(&v).is_file() => std::fs::read_to_string(&v).map_err(|e| format!("couldn't read the key file: {e}"))?,
        Ok(v) if !v.trim().is_empty() => v,
        _ => std::fs::read_to_string(default_key_path()).map_err(|e| format!("no STRATA_UPDATE_KEY and couldn't read {}: {e}", default_key_path().display()))?,
    };
    let seed = sig::b64_decode(text.trim()).ok_or("the private key isn't valid base64")?;
    let seed: [u8; 32] = seed.try_into().map_err(|_| "the private key isn't 32 bytes".to_owned())?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The platform keys and their files, as KEY=FILE arguments.
fn manifest(version: &str, notes: &Path, out: &Path, base_url: &str, files: &[String]) -> Result<(), String> {
    if version.starts_with('v') || version.is_empty() {
        return Err(format!("version {version:?}: give it without a leading v"));
    }
    if !base_url.starts_with("https://") {
        return Err("BASE_URL must be https".into());
    }
    let key = load_key()?;
    let notes = std::fs::read_to_string(notes).map_err(|e| format!("couldn't read {}: {e}", notes.display()))?;
    let mut platforms = serde_json::Map::new();
    for spec in files {
        let (platform, file) = spec.split_once('=').ok_or_else(|| format!("{spec}: expected KEY=FILE"))?;
        if !["macos-universal", "macos-arm64", "windows-x64"].contains(&platform) {
            return Err(format!("unknown platform {platform}"));
        }
        let path = Path::new(file);
        let f = std::fs::File::open(path).map_err(|e| format!("couldn't open {file}: {e}"))?;
        let (sha256, size) = sig::sha256_hex(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
        let signature = sig::b64_encode(&key.sign(&sig::message(platform, version, &sha256, size)).to_bytes());
        let name = path.file_name().and_then(|n| n.to_str()).ok_or("bad file name")?;
        let url = format!("{}/{}", base_url.trim_end_matches('/'), name);
        platforms.insert(platform.into(), serde_json::json!({ "url": url, "size": size, "sha256": sha256, "signature": signature }));
        eprintln!("signed {platform}: {name} ({size} bytes, sha256 {sha256})");
    }
    if platforms.is_empty() {
        return Err("no files to sign".into());
    }
    let doc = serde_json::json!({
        "version": version,
        "pub_date": now_rfc3339(),
        "notes": notes,
        "platforms": platforms,
    });
    std::fs::write(out, serde_json::to_string_pretty(&doc).unwrap() + "\n").map_err(|e| format!("couldn't write {}: {e}", out.display()))?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

/// Check a manifest against the files in DIR with the key's public half,
/// as the app will.
fn verify(manifest: &Path, dir: &Path) -> Result<(), String> {
    let key = load_key()?.verifying_key().to_bytes();
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let version = doc["version"].as_str().ok_or("no version")?;
    let platforms = doc["platforms"].as_object().ok_or("no platforms")?;
    for (platform, entry) in platforms {
        let url = entry["url"].as_str().ok_or("no url")?;
        let name = url.rsplit('/').next().unwrap_or_default();
        let f = std::fs::File::open(dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
        let (sha256, size) = sig::sha256_hex(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
        if Some(sha256.as_str()) != entry["sha256"].as_str() || Some(size) != entry["size"].as_u64() {
            return Err(format!("{platform}: {name} doesn't match the manifest's size or sha256"));
        }
        sig::verify(&key, &sig::message(platform, version, &sha256, size), entry["signature"].as_str().unwrap_or_default()).map_err(|e| format!("{platform}: {e}"))?;
        println!("ok {platform} {name}");
    }
    Ok(())
}

/// UTC now as 2026-10-14T17:43:00Z.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
