use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

fn main() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut sources = vec![workspace.join("Cargo.toml"), workspace.join("Cargo.lock")];
    let crates = workspace.join("crates");
    println!("cargo:rerun-if-changed={}", crates.display());
    let mut pending: Vec<PathBuf> = vec![crates];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext == "rs" || ext == "ts" || ext == "toml")
            {
                sources.push(path);
            }
        }
    }
    sources.sort();
    let mut hasher = DefaultHasher::new();
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
        hasher.write(
            source
                .strip_prefix(&workspace)
                .unwrap_or(source)
                .as_os_str()
                .as_encoded_bytes(),
        );
        hasher.write(&std::fs::read(source).unwrap_or_default());
    }
    println!("cargo:rustc-env=MIRA_BUILD={:016x}", hasher.finish());
}
