use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use mira_client::{Direction, Event};
use mira_protocol::Message;
use serde::{Deserialize, Serialize};

use crate::view::time_of_day;

const CUT_AT: usize = 160;

#[derive(Clone)]
pub struct Recorder(Arc<Mutex<File>>);

#[derive(Serialize, Deserialize)]
struct Recorded {
    t: u64,
    dir: Way,
    text: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
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

pub fn view(path: &Path, cmds: &[String], full: bool) -> anyhow::Result<()> {
    let file = File::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let packet: Recorded = serde_json::from_str(&line)
            .map_err(|e| anyhow::anyhow!("{}:{}: {e}", path.display(), n + 1))?;
        if let Some(shown) = shown(&packet, cmds, full) {
            println!("{shown}");
        }
    }
    Ok(())
}

fn shown(packet: &Recorded, cmds: &[String], full: bool) -> Option<String> {
    let cmd = command(packet.dir, &packet.text);
    if !cmds.is_empty() && !cmd.as_ref().is_some_and(|c| cmds.iter().any(|w| w == c)) {
        return None;
    }
    let arrow = match packet.dir {
        Way::In => "<",
        Way::Out => ">",
    };
    let mut text = packet.text.clone();
    if !full && text.chars().count() > CUT_AT {
        text = text.chars().take(CUT_AT).collect::<String>() + "…";
    }
    Some(format!("{}  {arrow} {text}", time_of_day(packet.t)))
}

fn command(dir: Way, text: &str) -> Option<String> {
    if dir == Way::Out && text.starts_with("%xt%") {
        let fields: Vec<&str> = text.split('%').collect();
        let cmd = *fields.get(3)?;
        return Some(if cmd == "cmd" {
            fields.get(5)?.to_string()
        } else {
            cmd.to_owned()
        });
    }
    if dir == Way::Out {
        let at = text.find("action='")? + "action='".len();
        return Some(text[at..].split('\'').next()?.to_owned());
    }
    match mira_protocol::parse(text).ok()? {
        Message::Xt(xt) => Some(xt.cmd().to_owned()),
        Message::Json(json) => Some(json.cmd),
        Message::Xml(xml) => Some(xml.action),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(dir: Way, text: &str) -> Recorded {
        Recorded {
            t: 0,
            dir,
            text: text.into(),
        }
    }

    #[test]
    fn view_names_each_packets_command_and_filters_by_it() {
        let cases = [
            (Way::Out, "%xt%zm%cmd%1%goto%player2%", "goto"),
            (Way::Out, "%xt%zm%moveToCell%1%Enter%Spawn%", "moveToCell"),
            (
                Way::Out,
                "<msg t='sys'><body action='verChk' r='0'></body></msg>",
                "verChk",
            ),
            (Way::In, "%xt%uotls%-1%player2%strFrame:r6%", "uotls"),
            (
                Way::In,
                r#"{"t":"xt","b":{"r":-1,"o":{"cmd":"moveToArea"}}}"#,
                "moveToArea",
            ),
        ];
        for (dir, text, cmd) in cases {
            assert_eq!(command(dir, text).as_deref(), Some(cmd), "{text}");
        }
        let goto = packet(Way::Out, "%xt%zm%cmd%1%goto%player2%");
        assert!(
            shown(&goto, &["goto".into()], false)
                .unwrap()
                .ends_with("> %xt%zm%cmd%1%goto%player2%")
        );
        assert_eq!(shown(&goto, &["uotls".into()], false), None);
        let long = packet(Way::In, &format!("%xt%server%-1%{}%", "x".repeat(300)));
        assert!(shown(&long, &[], false).unwrap().ends_with('…'));
        assert!(!shown(&long, &[], true).unwrap().ends_with('…'));
    }

    #[test]
    fn a_recording_reads_back_in_order() {
        let path =
            std::env::temp_dir().join(format!("mira-record-tests-{}.jsonl", std::process::id()));
        let recorder = Recorder::create(&path).unwrap();
        recorder.write(Direction::Out, "%xt%zm%cmd%1%goto%player2%".into());
        recorder.write(Direction::In, "%xt%uotls%-1%player2%strFrame:r6%".into());
        let lines: Vec<Recorded> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].dir == Way::Out && lines[1].dir == Way::In);
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        view(&path, &[], false).unwrap();
    }
}
