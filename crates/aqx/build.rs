use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let dist = out.join("web");
    if !build_web(&workspace.join("web"), &dist) {
        let _ = std::fs::remove_dir_all(&dist);
        println!("cargo:rerun-if-changed={}", dist.display());
    }
    embed_web(&dist, &out);
}

fn build_web(web: &Path, dist: &Path) -> bool {
    for entry in std::fs::read_dir(web).into_iter().flatten().flatten() {
        if !matches!(entry.file_name().to_str(), Some("node_modules" | "dist")) {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let installed = modified(&web.join("node_modules/.package-lock.json"));
    let current = installed.is_some() && installed >= modified(&web.join("package-lock.json"));
    let dist = dist.to_str().expect("OUT_DIR is UTF-8");
    (current || npm(web, &["ci"]))
        && npm(
            web,
            &["run", "build", "--", "--outDir", dist, "--emptyOutDir"],
        )
}

fn npm(dir: &Path, args: &[&str]) -> bool {
    let output = match Command::new("npm").args(args).current_dir(dir).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "cargo:warning=npm isn't installed, so aqx web will only show a note. Install Node 24 or newer, then build aqx again."
            );
            return false;
        }
        Err(error) => panic!("couldn't run npm {}: {error}", args.join(" ")),
    };
    assert!(
        output.status.success(),
        "npm {} failed\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn embed_web(dist: &Path, out: &Path) {
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
    let mut table = String::from("&[\n");
    for file in &files {
        let served = file.strip_prefix(dist).expect("listed under dist");
        let absolute = std::fs::canonicalize(file).expect("listed files exist");
        table.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            format!("/{}", served.display()),
            absolute.display().to_string()
        ));
    }
    table.push(']');
    std::fs::write(out.join("web_assets.rs"), table).expect("OUT_DIR is writable");
}
