use std::fs::File;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use aqx_client::{Direction, Event};
use serde::Serialize;

#[derive(Clone)]
pub struct Recorder(Arc<Mutex<File>>);

#[derive(Serialize)]
struct Recorded {
    t: u64,
    dir: Way,
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Way {
    In,
    Out,
}

impl Recorder {
    pub fn create(path: &Path) -> std::io::Result<Recorder> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        Ok(Recorder(Arc::new(Mutex::new(file))))
    }

    pub fn follow(&self, events: Receiver<Event>) {
        let recorder = self.clone();
        std::thread::spawn(move || {
            for event in events {
                if let Event::Packet { direction, text } = event {
                    recorder.write(direction, text);
                }
            }
        });
    }

    fn write(&self, direction: Direction, text: String) {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let dir = match direction {
            Direction::In => Way::In,
            Direction::Out => Way::Out,
        };
        let mut line =
            serde_json::to_string(&Recorded { t, dir, text }).expect("packets serialize");
        line.push('\n');
        let mut file = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_reads_back_in_order() {
        let path =
            std::env::temp_dir().join(format!("aqx-record-tests-{}.jsonl", std::process::id()));
        let recorder = Recorder::create(&path).unwrap();
        recorder.write(Direction::Out, "%xt%zm%cmd%1%goto%player2%".into());
        recorder.write(Direction::In, "%xt%uotls%-1%player2%strFrame:r6%".into());
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
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
    }
}
