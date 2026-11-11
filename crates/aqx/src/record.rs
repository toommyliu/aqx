use std::fs::File;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use aqx_client::{Direction, Event, Handle, Update};
use serde::Serialize;

#[derive(Clone)]
pub struct Recorder {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

#[derive(Serialize)]
struct Recorded<'a> {
    t: u64,
    dir: Way,
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Way {
    In,
    Out,
}

pub fn default_path(account: &str) -> std::io::Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| std::io::Error::other("no data folder to keep recordings in"))?
        .join("aqx")
        .join("recordings");
    std::fs::create_dir_all(&dir)?;
    let account = account.replace(['/', '\\'], "_");
    Ok(dir.join(format!("{account}-{}.jsonl", local_stamp())))
}

fn local_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

impl Recorder {
    pub fn create(path: &Path) -> std::io::Result<Recorder> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(Recorder {
            path: path.to_path_buf(),
            file: Arc::new(Mutex::new(file)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn follow(&self, events: Receiver<Event>) {
        let file = Arc::clone(&self.file);
        std::thread::spawn(move || {
            for event in events {
                if let Event::Packet { direction, text } = event {
                    write(&file, direction, &text);
                }
            }
        });
    }

    pub fn watch(&self, session: &Handle, mut seen: impl FnMut(Direction, &str) + Send + 'static) {
        let file = Arc::downgrade(&self.file);
        let updates = session.updates();
        std::thread::spawn(move || {
            for update in updates {
                let Some(file) = file.upgrade() else {
                    return;
                };
                if let Update::Packet(direction, text) = update {
                    write(&file, direction, &text);
                    seen(direction, &text);
                }
            }
        });
    }
}

fn write(file: &Mutex<File>, direction: Direction, text: &str) {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let dir = match direction {
        Direction::In => Way::In,
        Direction::Out => Way::Out,
    };
    let mut line = serde_json::to_string(&Recorded { t, dir, text }).expect("packets serialize");
    line.push('\n');
    let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = file.write_all(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_reads_back_in_order() {
        let path =
            std::env::temp_dir().join(format!("aqx-record-tests-{}.jsonl", std::process::id()));
        let recorder = Recorder::create(&path).unwrap();
        write(&recorder.file, Direction::Out, "%xt%zm%cmd%1%goto%player2%");
        write(
            &recorder.file,
            Direction::In,
            "%xt%uotls%-1%player2%strFrame:r6%",
        );
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            (&lines[0]["dir"], &lines[1]["dir"]),
            (&"out".into(), &"in".into())
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn recording_over_a_file_others_can_read_makes_it_private() {
        let path =
            std::env::temp_dir().join(format!("aqx-record-shared-{}.jsonl", std::process::id()));
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        Recorder::create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(mode & 0o777, 0o600);
    }
}
