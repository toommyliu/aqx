use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

fn main() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let workspace = Path::new(&manifest).join("../..");
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
    println!("cargo:rustc-env=AQX_BUILD={:016x}", hasher.finish());
    embed_web(&workspace.join("web/dist"));
}

fn embed_web(dist: &Path) {
    println!("cargo:rerun-if-changed={}", dist.display());
    let mut files = Vec::new();
    let mut pending = vec![dist.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        println!(
            "cargo:warning=the web UI isn't built, so aqx web will only show a note. Run npm --prefix web ci && npm --prefix web run build, then build aqx again."
        );
    }
    let mut table = String::from("&[\n");
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
        let served = file.strip_prefix(dist).expect("listed under dist");
        let absolute = std::fs::canonicalize(file).expect("listed files exist");
        table.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            format!("/{}", served.display()),
            absolute.display().to_string()
        ));
    }
    table.push(']');
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("web_assets.rs"), table).expect("OUT_DIR is writable");
}
