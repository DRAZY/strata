//! Precompiles the bundled syntax set: bat's extra syntaxes (via two-face)
//! plus Strata's own `assets/syntaxes/*.sublime-syntax`, linked once here and
//! dumped to a binary blob the app loads lazily in about a millisecond.
//! (Linking the set at runtime instead costs a few hundred milliseconds.)

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=assets/syntaxes");
    println!("cargo:rerun-if-changed=build.rs");
    let mut builder = two_face::syntax::extra_newlines().into_builder();
    builder.add_from_folder("assets/syntaxes", true).expect("assets/syntaxes parse");
    let set = builder.build();
    let out = std::env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);
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
}
