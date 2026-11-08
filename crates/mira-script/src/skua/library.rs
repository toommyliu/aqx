use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

const ARCHIVE: &str = "https://codeload.github.com/auqw/Scripts/tar.gz/refs/heads/Skua";
const DOWNLOAD_LIMIT: u64 = 64 << 20;
const VERSION: &str = ".mira-version";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Updated {
    Current,
    To(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub commit: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub path: String,
}

#[derive(Serialize, Deserialize)]
struct Version {
    etag: Option<String>,
    commit: String,
}

#[derive(Deserialize)]
struct Listed {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    path: String,
}

pub fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

pub fn scripts(skua: &Path) -> PathBuf {
    skua.join("Scripts")
}

pub fn open(skua: &Path) -> Result<Option<Library>, String> {
    let scripts = scripts(skua);
    let Some(version) = version(&scripts)? else {
        return Ok(None);
    };
    let listed = fs::read(scripts.join("scripts.json"))
        .map_err(|e| format!("couldn't read the Skua script list: {e}"))?;
    let listed: Vec<Listed> = serde_json::from_slice(&listed)
        .map_err(|e| format!("couldn't read the Skua script list: {e}"))?;
    let mut entries: Vec<Entry> = listed
        .into_iter()
        .filter(|l| !storage(&l.path))
        .map(Entry::from)
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some(Library {
        commit: version.commit,
        entries,
    }))
}

fn storage(path: &str) -> bool {
    path.split('/').any(|part| part.starts_with("Core"))
}

impl From<Listed> for Entry {
    fn from(listed: Listed) -> Entry {
        let known = |text: Option<String>| text.filter(|t| !t.is_empty() && t != "null");
        let mut tags: Vec<String> = Vec::new();
        for tag in listed.tags.into_iter().filter_map(|t| known(Some(t))) {
            let tag = tag.to_lowercase();
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        let stem = Path::new(&listed.path)
            .file_stem()
            .map_or_else(|| listed.path.clone(), |s| s.to_string_lossy().into_owned());
        Entry {
            name: known(listed.name).unwrap_or(stem),
            description: known(listed.description).unwrap_or_default(),
            tags,
            path: listed.path,
        }
    }
}

enum Word<'a> {
    Text(&'a str),
    Tag(&'a str),
    TagPrefix(&'a str),
}

impl Library {
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let query = query.to_lowercase();
        let words: Vec<Word> = query
            .split_whitespace()
            .filter_map(|word| match word.strip_prefix('#') {
                Some("") => None,
                Some(tag) if self.entries.iter().any(|e| e.tags.iter().any(|t| t == tag)) => {
                    Some(Word::Tag(tag))
                }
                Some(prefix) => Some(Word::TagPrefix(prefix)),
                None => Some(Word::Text(word)),
            })
            .collect();
        self.entries
            .iter()
            .filter(|e| words.iter().all(|word| e.has(word)))
            .collect()
    }
}

impl Entry {
    fn has(&self, word: &Word) -> bool {
        match *word {
            Word::Tag(tag) => self.tags.iter().any(|t| t == tag),
            Word::TagPrefix(prefix) => self.tags.iter().any(|t| t.starts_with(prefix)),
            Word::Text(text) => [&self.name, &self.path, &self.description]
                .into_iter()
                .chain(&self.tags)
                .any(|field| field.to_lowercase().contains(text)),
        }
    }
}

pub fn update(skua: &Path) -> Result<Updated, String> {
    update_from(skua, ARCHIVE)
}

fn update_from(skua: &Path, url: &str) -> Result<Updated, String> {
    let failed = |e: io::Error| format!("couldn't update the Skua scripts: {e}");
    fs::create_dir_all(skua).map_err(failed)?;
    let lock = File::create(skua.join(".update.lock")).map_err(failed)?;
    lock.lock().map_err(failed)?;
    let scripts = scripts(skua);
    let staged = skua.join(".Scripts.new");
    let old = skua.join(".Scripts.old");
    if !scripts.exists() && old.exists() {
        fs::rename(&old, &scripts).map_err(failed)?;
    }
    remove(&staged).map_err(failed)?;
    remove(&old).map_err(failed)?;
    let current = version(&scripts)?;
    if current.is_none() && scripts.exists() {
        return Err(format!(
            "{} wasn't downloaded by mira, so mira won't replace it. Move it somewhere else, then update again",
            scripts.display()
        ));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    let mut request = agent.get(url);
    if let Some(etag) = current.as_ref().and_then(|v| v.etag.as_deref()) {
        request = request.header("If-None-Match", etag);
    }
    let response = request
        .call()
        .map_err(|e| format!("couldn't download the Skua scripts: {e}"))?;
    match response.status().as_u16() {
        304 if current.is_some() => return Ok(Updated::Current),
        200 => {}
        status => {
            return Err(format!(
                "couldn't download the Skua scripts: GitHub answered {status}"
            ));
        }
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response
        .into_body()
        .into_with_config()
        .limit(DOWNLOAD_LIMIT)
        .reader();
    let commit =
        unpack(body, &staged).map_err(|e| format!("couldn't unpack the Skua scripts: {e}"))?;
    let version = serde_json::to_vec(&Version {
        etag,
        commit: commit.clone(),
    })
    .expect("a version serializes");
    fs::write(staged.join(VERSION), version).map_err(failed)?;
    if scripts.exists() {
        fs::rename(&scripts, &old).map_err(failed)?;
    }
    fs::rename(&staged, &scripts).map_err(failed)?;
    remove(&old).map_err(failed)?;
    Ok(Updated::To(commit))
}

fn unpack(body: impl io::Read, into: &Path) -> io::Result<String> {
    fs::create_dir_all(into)?;
    let mut archive = tar::Archive::new(GzDecoder::new(body));
    let mut commit = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            for extension in entry.pax_extensions()?.into_iter().flatten() {
                let extension = extension?;
                if extension.key_bytes() == b"comment" {
                    commit = Some(String::from_utf8_lossy(extension.value_bytes()).into_owned());
                }
            }
            continue;
        }
        let path = entry.path()?.into_owned();
        let relative: PathBuf = path.components().skip(1).collect();
        if !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(io::Error::other(format!(
                "{} points outside the library",
                path.display()
            )));
        }
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = into.join(&relative);
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&target)?;
        }
    }
    commit.ok_or_else(|| io::Error::other("the download doesn't name its commit"))
}

fn version(scripts: &Path) -> Result<Option<Version>, String> {
    match fs::read(scripts.join(VERSION)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("couldn't read {}: {e}", scripts.join(VERSION).display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "couldn't read {}: {e}",
            scripts.join(VERSION).display()
        )),
    }
}

fn remove(dir: &Path) -> io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    struct Upstream {
        url: String,
        archive: Arc<Mutex<Vec<u8>>>,
        etag: Arc<Mutex<String>>,
        asked: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl Upstream {
        fn start(archive: Vec<u8>) -> Upstream {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/Scripts.tar.gz", listener.local_addr().unwrap());
            let archive = Arc::new(Mutex::new(archive));
            let etag = Arc::new(Mutex::new("\"one\"".to_owned()));
            let asked = Arc::new(Mutex::new(Vec::new()));
            let (served, tag, log) = (archive.clone(), etag.clone(), asked.clone());
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    let mut sent = None;
                    for line in BufReader::new(&stream).lines() {
                        let line = line.unwrap();
                        if line.is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(": ")
                            && name.eq_ignore_ascii_case("if-none-match")
                        {
                            sent = Some(value.to_owned());
                        }
                    }
                    log.lock().unwrap().push(sent.clone());
                    let tag = tag.lock().unwrap().clone();
                    if sent.as_deref() == Some(tag.as_str()) {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 304 Not Modified\r\nETag: {tag}\r\nConnection: close\r\n\r\n"
                        );
                        continue;
                    }
                    let body = served.lock().unwrap().clone();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nETag: {tag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                }
            });
            Upstream {
                url,
                archive,
                etag,
                asked,
            }
        }

        fn publish(&self, archive: Vec<u8>, etag: &str) {
            *self.archive.lock().unwrap() = archive;
            *self.etag.lock().unwrap() = etag.to_owned();
        }
    }

    fn archive(commit: &str, files: &[(&str, &str)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let record = format!(" comment={commit}\n");
        let mut length = record.len() + 2;
        if format!("{length}{record}").len() != length {
            length += 1;
        }
        let pax = format!("{length}{record}");
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::XGlobalHeader);
        header.set_path("pax_global_header").unwrap();
        header.set_size(pax.len() as u64);
        header.set_cksum();
        tar.append(&header, pax.as_bytes()).unwrap();
        for (path, text) in files {
            let mut header = tar::Header::new_ustar();
            let name = format!("Scripts-Skua/{path}");
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            tar.append(&header, text.as_bytes()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    const LISTED: &str = r#"[
        {"name": "null", "description": "null", "tags": ["null"], "path": "CoreBots.cs"},
        {"name": "Legion", "description": "", "tags": [], "path": "Legion/CoreLegion.cs"},
        {"name": "Task Extensions", "description": "", "tags": [], "path": "Core/TaskExtensions.cs"},
        {"name": "null", "description": "null", "tags": ["null"], "path": "Ultras/Xyfrag.cs"},
        {"name": "Void Aura", "description": "Farms Void Auras", "tags": ["farm", "", "Farm"], "path": "Farm/VoidAura.cs"}
    ]"#;

    fn skua() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mira-library-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn read(skua: &Path, path: &str) -> Option<String> {
        fs::read_to_string(scripts(skua).join(path)).ok()
    }

    #[test]
    fn an_update_downloads_once_and_then_only_when_upstream_changes() {
        let skua = skua();
        let first = archive(
            "aaaaaaa",
            &[
                ("scripts.json", LISTED),
                ("CoreBots.cs", "v1"),
                ("Farm/VoidAura.cs", "farm"),
            ],
        );
        let upstream = Upstream::start(first);
        assert_eq!(open(&skua), Ok(None));

        assert_eq!(
            update_from(&skua, &upstream.url),
            Ok(Updated::To("aaaaaaa".into()))
        );
        assert_eq!(read(&skua, "Farm/VoidAura.cs").as_deref(), Some("farm"));
        let library = open(&skua).unwrap().unwrap();
        assert_eq!(library.commit, "aaaaaaa");
        assert_eq!(
            library.entries,
            [
                Entry {
                    name: "Void Aura".into(),
                    description: "Farms Void Auras".into(),
                    tags: vec!["farm".into()],
                    path: "Farm/VoidAura.cs".into()
                },
                Entry {
                    name: "Xyfrag".into(),
                    description: "".into(),
                    tags: vec![],
                    path: "Ultras/Xyfrag.cs".into()
                },
            ]
        );

        assert_eq!(update_from(&skua, &upstream.url), Ok(Updated::Current));

        upstream.publish(
            archive("bbbbbbb", &[("scripts.json", "[]"), ("CoreBots.cs", "v2")]),
            "\"two\"",
        );
        assert_eq!(
            update_from(&skua, &upstream.url),
            Ok(Updated::To("bbbbbbb".into()))
        );
        assert_eq!(read(&skua, "CoreBots.cs").as_deref(), Some("v2"));
        assert_eq!(read(&skua, "Farm/VoidAura.cs"), None);
        assert_eq!(
            *upstream.asked.lock().unwrap(),
            [None, Some("\"one\"".into()), Some("\"one\"".into())]
        );
        let _ = fs::remove_dir_all(&skua);
    }

    #[test]
    fn a_failed_or_unsafe_download_keeps_the_current_library() {
        let skua = skua();
        let upstream = Upstream::start(archive(
            "aaaaaaa",
            &[("scripts.json", "[]"), ("CoreBots.cs", "v1")],
        ));
        update_from(&skua, &upstream.url).unwrap();

        upstream.publish(b"not a tarball".to_vec(), "\"two\"");
        assert!(update_from(&skua, &upstream.url).is_err());
        upstream.publish(
            archive("bbbbbbb", &[("CoreBots.cs", "v2"), ("../escaped.cs", "x")]),
            "\"three\"",
        );
        let escaped = update_from(&skua, &upstream.url).unwrap_err();
        assert!(escaped.contains("outside the library"), "{escaped}");

        assert!(!skua.join("escaped.cs").exists());
        assert_eq!(read(&skua, "CoreBots.cs").as_deref(), Some("v1"));
        assert_eq!(open(&skua).unwrap().unwrap().commit, "aaaaaaa");
        let _ = fs::remove_dir_all(&skua);
    }

    #[test]
    fn an_update_never_replaces_a_folder_mira_did_not_download() {
        let skua = skua();
        fs::create_dir_all(scripts(&skua)).unwrap();
        fs::write(scripts(&skua).join("Mine.cs"), "mine").unwrap();
        let upstream = Upstream::start(archive("aaaaaaa", &[("CoreBots.cs", "v1")]));

        let refused = update_from(&skua, &upstream.url).unwrap_err();

        assert!(refused.contains("wasn't downloaded by mira"), "{refused}");
        assert_eq!(read(&skua, "Mine.cs").as_deref(), Some("mine"));
        assert!(upstream.asked.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(&skua);
    }

    #[test]
    fn an_update_recovers_a_library_left_aside_by_an_interrupted_swap() {
        let skua = skua();
        let upstream = Upstream::start(archive(
            "aaaaaaa",
            &[("scripts.json", "[]"), ("CoreBots.cs", "v1")],
        ));
        update_from(&skua, &upstream.url).unwrap();
        fs::rename(scripts(&skua), skua.join(".Scripts.old")).unwrap();
        fs::create_dir_all(skua.join(".Scripts.new")).unwrap();

        assert_eq!(update_from(&skua, &upstream.url), Ok(Updated::Current));

        assert_eq!(read(&skua, "CoreBots.cs").as_deref(), Some("v1"));
        assert!(!skua.join(".Scripts.old").exists());
        assert!(!skua.join(".Scripts.new").exists());
        let _ = fs::remove_dir_all(&skua);
    }

    #[test]
    fn a_search_matches_every_word_and_a_hash_word_matches_tags() {
        let entry = |name: &str, tags: &[&str]| Entry {
            name: name.into(),
            description: format!("Farms {name}"),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            path: format!("Farm/{}.cs", name.replace(' ', "")),
        };
        let library = Library {
            commit: "aaaaaaa".into(),
            entries: vec![
                entry("Void Aura", &["nulgath", "farm"]),
                entry("Rep Shop", &["rep"]),
                entry("Reputation Grind", &["reputation"]),
            ],
        };
        let names = |query: &str| -> Vec<&str> {
            library
                .search(query)
                .iter()
                .map(|e| e.name.as_str())
                .collect()
        };
        let all = ["Void Aura", "Rep Shop", "Reputation Grind"];
        assert_eq!(names(""), all);
        assert_eq!(names("void NULGATH"), ["Void Aura"]);
        assert_eq!(names("farm/ rep"), ["Rep Shop", "Reputation Grind"]);
        assert!(names("void legion").is_empty());
        assert_eq!(names("#Nulgath aura"), ["Void Aura"]);
        assert_eq!(
            names("#rep"),
            ["Rep Shop"],
            "a whole tag matches only itself"
        );
        assert_eq!(names("#re"), ["Rep Shop", "Reputation Grind"]);
        assert_eq!(names("#repu"), ["Reputation Grind"]);
        assert!(names("#void").is_empty(), "a tag search skips the name");
        assert!(names("#xyz").is_empty());
        assert_eq!(names("#"), all);
    }
}
