//! Precompiles the bundled syntax set: bat's extra syntaxes (via two-face)
//! plus Strata's own `assets/syntaxes/*.sublime-syntax`, linked once here and
//! dumped to a binary blob the app loads lazily in about a millisecond.
//! (Linking the set at runtime instead costs a few hundred milliseconds.)
//!
//! Also records the git commit being built (`STRATA_GIT_HASH`, for crash
//! reports). Packagers building outside git can set `STRATA_GIT_HASH` themselves.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    println!("cargo:rerun-if-changed=build.rs");
    git_hash();
    let out = std::env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);
    // This script also reruns when the commit changes; the slow part only
    // when what it's made from did.
    let stamp = format!("{:x}", inputs_hash());
    let stamp_file = out.join("syntaxes.stamp");
    if std::fs::read_to_string(&stamp_file).is_ok_and(|s| s == stamp) && out.join("syntaxes.bin").exists() && out.join("syntax-licenses.txt").exists() {
        return;
    }
    let mut builder = two_face::syntax::extra_newlines().into_builder();
    builder.add_from_folder("assets/syntaxes", true).expect("assets/syntaxes parse");
    let set = builder.build();
    syntect::dumps::dump_to_uncompressed_file(&set, out.join("syntaxes.bin")).expect("write syntax dump");

    // License texts the bundled grammars require us to reproduce (Help > Licenses).
    let mut text = String::from(
        "Syntax definitions collected by the bat project (github.com/sharkdp/bat),\n\
         packaged by two-face. Licenses that ask to be reproduced:\n",
    );
    for license in two_face::acknowledgement::listing().for_syntaxes() {
        let path = license.rel_path.display().to_string();
        let path = path.strip_prefix("syntaxes/").unwrap_or(&path);
        text.push_str(&format!("\n── {path} ──\n\n{}\n", license.text.trim_end()));
    }
    std::fs::write(out.join("syntax-licenses.txt"), text).expect("write syntax licenses");
    std::fs::write(stamp_file, stamp).expect("write syntax stamp");
}

/// Everything the syntax dump is made from: this script, the lock file
/// (two-face's and syntect's versions) and `assets/syntaxes`.
fn inputs_hash() -> u64 {
    fn walk(dir: &Path, h: &mut std::collections::hash_map::DefaultHasher) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, h);
            } else {
                p.hash(h);
                std::fs::read(&p).unwrap_or_default().hash(h);
            }
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for f in ["build.rs", "Cargo.lock"] {
        std::fs::read(f).unwrap_or_default().hash(&mut h);
    }
    walk(Path::new("assets/syntaxes"), &mut h);
    h.finish()
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8(out.stdout).ok()?.trim().to_owned()).filter(|s| !s.is_empty())
}

fn git_hash() {
    println!("cargo:rerun-if-env-changed=STRATA_GIT_HASH");
    if let Ok(hash) = std::env::var("STRATA_GIT_HASH") {
        println!("cargo:rustc-env=STRATA_GIT_HASH={hash}");
        return;
    }
    let Some(hash) = git(&["rev-parse", "--short=12", "HEAD"]) else { return };
    // Rerun when HEAD moves: HEAD itself, the branch it names, packed refs.
    let mut watch = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    watch.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            let path = std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    println!("cargo:rustc-env=STRATA_GIT_HASH={hash}");
}
