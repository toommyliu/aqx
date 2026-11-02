use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let mut inputs = vec![root.join("Cargo.toml"), root.join("Cargo.lock")];
    for krate in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        inputs.push(krate.path().join("Cargo.toml"));
        inputs.push(krate.path().join("src"));
    }
    let mut files = Vec::new();
    for input in &inputs {
        println!("cargo:rerun-if-changed={}", input.display());
        collect(input, &mut files);
    }
    files.sort();
    let mut hasher = DefaultHasher::new();
    for file in &files {
        file.strip_prefix(&root).unwrap_or(file).hash(&mut hasher);
        std::fs::read(file).unwrap().hash(&mut hasher);
    }
    println!("cargo:rustc-env=AQX_BUILD={:016x}", hasher.finish());
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            collect(&entry.path(), files);
        }
    } else if path.is_file() {
        files.push(path.to_path_buf());
    }
}
