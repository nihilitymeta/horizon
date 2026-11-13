//! Collects the build's feature registry, one `registry/<id>.json` per toggleable feature
//! (`src/mod_registry.rs`), into a generated list. A feature branch registers itself by adding a
//! file, so no two branches ever edit the same list. std only: the engine crate may not depend on
//! the mod API generator (docs/layers.md).
use std::{env, fs, path::PathBuf};

fn main() {
    feature_registry();
}

fn feature_registry() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("registry");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut out = String::from(
        "/// Every `registry/<id>.json` of this build: its file stem and its text.\n\
         pub const RAW: &[(&str, &str)] = &[\n",
    );
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let id = file.file_stem().unwrap().to_string_lossy();
        out.push_str(&format!(
            "    ({id:?}, include_str!({:?})),\n",
            file.display().to_string()
        ));
    }
    out.push_str("];\n");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("mod_registry.rs"), out).unwrap();
}
