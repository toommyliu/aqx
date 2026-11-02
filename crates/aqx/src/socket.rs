use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::hub::{Command, Hub, LogLine, Snapshot};

const TICK: Duration = Duration::from_millis(250);
const FRESH: Duration = Duration::from_millis(300);
const NAME_TRIES: usize = 100;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Info,
    Snapshot,
    Subscribe,
    Logs {
        bot: Option<String>,
        #[serde(default)]
        follow: bool,
    },
    Ctl {
        bot: String,
        #[serde(flatten)]
        command: Command,
    },
}

pub fn dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return runtime.join("aqx");
    }
    if cfg!(target_os = "macos")
        && let Some(tmp) = std::env::var_os("TMPDIR").map(PathBuf::from).filter(|p| p.is_absolute())
    {
        return tmp.join("aqx");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    PathBuf::from(format!("/tmp/aqx-{user}"))
}

pub struct Listening {
    path: PathBuf,
}

impl Drop for Listening {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn private(dir: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me || meta.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} must be a directory only you can open (mode 700); remove it and try again", dir.display()));
    }
    Ok(())
}

pub fn prepare_dir() -> Result<PathBuf, String> {
    let dir = dir();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    private(&dir)?;
    Ok(dir)
}

pub fn bind(name: &str, unique: bool) -> Result<(String, UnixListener, Listening), String> {
    let dir = prepare_dir()?;
    for n in 1..=NAME_TRIES {
        let taken = if n == 1 { name.to_string() } else { format!("{name}-{n}") };
        let path = dir.join(format!("{taken}.sock"));
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                if unique {
                    continue;
                }
                return Err(format!("an aqx process named {taken} is already running; its socket is {}", path.display()));
            }
            let _ = fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok((taken, listener, Listening { path }));
    }
    Err(format!("{NAME_TRIES} aqx processes named {name} are already running"))
}

pub fn serve(listener: UnixListener, hub: Arc<Hub>, config: Option<PathBuf>) -> Result<(), String> {
    thread::Builder::new()
        .name("control".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let (hub, config) = (hub.clone(), config.clone());
                let _ = thread::Builder::new().name("client".into()).spawn(move || serve_client(&hub, config.as_deref(), conn));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn send(out: &mut UnixStream, value: &Value) -> bool {
    writeln!(out, "{value}").is_ok()
}

fn serve_client(hub: &Arc<Hub>, config: Option<&Path>, conn: UnixStream) {
    let Ok(mut out) = conn.try_clone() else { return };
    for line in BufReader::new(conn).lines() {
        let Ok(line) = line else { return };
        let reply = match serde_json::from_str::<Request>(&line) {
            Err(e) => json!({ "type": "error", "message": format!("unreadable request: {e}") }),
            Ok(Request::Info) => json!({ "type": "info", "name": hub.name, "config": config, "bots": names(hub) }),
            Ok(Request::Snapshot) => json!({ "type": "snapshot", "name": hub.name, "bots": snapshots(hub) }),
            Ok(Request::Ctl { bot, command }) => match hub.cell(&bot) {
                None => json!({ "type": "error", "message": format!("no bot named {bot} in {}", hub.name) }),
                Some(cell) => match cell.send(command) {
                    Ok(()) => json!({ "type": "ok" }),
                    Err(e) => json!({ "type": "error", "message": e }),
                },
            },
            Ok(Request::Subscribe) => return subscribe(hub, &mut out),
            Ok(Request::Logs { bot, follow }) => return logs(hub, bot.as_deref(), follow, &mut out),
        };
        if !send(&mut out, &reply) {
            return;
        }
    }
}

fn snapshots(hub: &Arc<Hub>) -> Vec<Snapshot> {
    let fresh = hub.cells.iter().all(|c| c.board().snapshot.is_some()) && hub.watched();
    if !fresh {
        let _watching = hub.watch();
        thread::sleep(FRESH);
    }
    hub.cells.iter().map(|c| c.board().snapshot.clone().unwrap_or_else(|| Snapshot { bot: c.name.clone(), phase: "STARTING".into(), ..Snapshot::default() })).collect()
}

fn names(hub: &Hub) -> Vec<&str> {
    hub.cells.iter().map(|c| c.name.as_str()).collect()
}

fn subscribe(hub: &Arc<Hub>, out: &mut UnixStream) {
    let _watching = hub.watch();
    if !send(out, &json!({ "type": "hello", "name": hub.name, "bots": names(hub) })) {
        return;
    }
    let mut shown: Vec<Option<Snapshot>> = vec![None; hub.cells.len()];
    let mut next: Vec<u64> = vec![0; hub.cells.len()];
    loop {
        for (i, cell) in hub.cells.iter().enumerate() {
            let (snapshot, lines) = {
                let board = cell.board();
                let lines: Vec<LogLine> = board.log.iter().filter(|l| l.seq >= next[i]).cloned().collect();
                (board.snapshot.clone(), lines)
            };
            for line in lines {
                next[i] = line.seq + 1;
                if !send(out, &json!({ "type": "log", "bot": cell.name, "line": line })) {
                    return;
                }
            }
            if snapshot.is_some() && snapshot != shown[i] {
                if !send(out, &json!({ "type": "snapshot", "snapshot": snapshot })) {
                    return;
                }
                shown[i] = snapshot;
            }
        }
        thread::sleep(TICK);
    }
}

fn logs(hub: &Arc<Hub>, bot: Option<&str>, follow: bool, out: &mut UnixStream) {
    let cells: Vec<_> = hub.cells.iter().filter(|c| bot.is_none_or(|b| c.name == b)).collect();
    if cells.is_empty() {
        send(out, &json!({ "type": "error", "message": format!("no bot named {} in {}", bot.unwrap_or_default(), hub.name) }));
        return;
    }
    let mut next = vec![0u64; cells.len()];
    loop {
        let mut lines: Vec<(&str, LogLine)> = Vec::new();
        for (i, cell) in cells.iter().enumerate() {
            let board = cell.board();
            let from = next[i];
            for line in board.log.iter().filter(|l| l.seq >= from) {
                next[i] = line.seq + 1;
                lines.push((&cell.name, line.clone()));
            }
        }
        lines.sort_by_key(|(_, l)| l.t);
        for (name, line) in lines {
            if !send(out, &json!({ "type": "log", "bot": name, "line": line })) {
                return;
            }
        }
        if !follow {
            send(out, &json!({ "type": "end" }));
            return;
        }
        thread::sleep(TICK);
    }
}

pub struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Conn {
    pub fn open(path: &Path) -> Result<Conn, String> {
        let stream = UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Conn { reader: BufReader::new(stream), writer })
    }

    pub fn send(&mut self, request: &Value) -> Result<(), String> {
        writeln!(self.writer, "{request}").map_err(|e| format!("the aqx process went away: {e}"))
    }

    pub fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => serde_json::from_str(&line).ok(),
        }
    }

    pub fn ask(path: &Path, request: &Value) -> Result<Value, String> {
        let mut conn = Conn::open(path)?;
        conn.send(request)?;
        let reply = conn.recv().ok_or_else(|| "the aqx process closed the connection".to_string())?;
        if reply["type"] == "error" {
            return Err(reply["message"].as_str().unwrap_or("error").to_string());
        }
        Ok(reply)
    }
}

pub fn processes() -> Result<Vec<(String, PathBuf)>, String> {
    let dir = dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    private(&dir)?;
    let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sock"))
        .filter(|p| UnixStream::connect(p).is_ok())
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    found.sort();
    Ok(found)
}

pub fn pick(name: Option<&str>) -> Result<(String, PathBuf), String> {
    let all = processes()?;
    match (name, &all[..]) {
        (Some(name), all) => all.iter().find(|(n, _)| n == name).cloned().ok_or_else(|| format!("no aqx process named {name}{}", listed(all))),
        (None, [one]) => Ok(one.clone()),
        (None, []) => Err("no aqx process is running".into()),
        (None, all) => Err(format!("several aqx processes are running; pick one{}", listed(all))),
    }
}

fn listed(all: &[(String, PathBuf)]) -> String {
    if all.is_empty() { String::new() } else { format!(": {}", all.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")) }
}

pub fn session_for(config: &Path) -> Result<Option<PathBuf>, String> {
    let Some(config) = config.to_str() else { return Ok(None) };
    Ok(processes()?.into_iter().map(|(_, path)| path).find(|path| Conn::ask(path, &json!({ "op": "info" })).is_ok_and(|info| info["config"] == config)))
}

pub fn bots(path: &Path) -> Result<Vec<String>, String> {
    let info = Conn::ask(path, &json!({ "op": "info" }))?;
    serde_json::from_value(info["bots"].clone()).map_err(|e| e.to_string())
}

pub fn snapshot(path: &Path) -> Result<Vec<Snapshot>, String> {
    let reply = Conn::ask(path, &json!({ "op": "snapshot" }))?;
    serde_json::from_value(reply["bots"].clone()).map_err(|e| e.to_string())
}

pub fn ctl(path: &Path, bot: &str, command: &Command) -> Result<(), String> {
    let mut request = serde_json::to_value(command).map_err(|e| e.to_string())?;
    request["op"] = json!("ctl");
    request["bot"] = json!(bot);
    Conn::ask(path, &request).map(|_| ())
}

pub fn find_bot(bot: &str) -> Result<PathBuf, String> {
    let all = processes()?;
    if all.is_empty() {
        return Err("no aqx process is running".into());
    }
    for (_, path) in &all {
        if snapshot(path).is_ok_and(|bots| bots.iter().any(|b| b.bot == bot)) {
            return Ok(path.clone());
        }
    }
    Err(format!("no running aqx process has a bot named {bot}"))
}

#[cfg(test)]
mod tests {
    use aqx_core::Control;

    use super::*;
    use crate::hub::{BotCell, Print};

    #[test]
    fn answers_snapshots_logs_and_commands() {
        let control = Control::new().unwrap();
        let cell = Arc::new(BotCell::local("main", control.stop_handle()));
        let name = format!("test-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, vec![cell.clone()], Print::Quiet));
        hub.say(&cell, "aqx", "hello");
        cell.board().online = true;
        let (taken, listener, listening) = bind(&name, false).unwrap();
        assert_eq!(taken, name);
        serve(listener, hub.clone(), Some("/srv/aqx.toml".into())).unwrap();
        assert!(bind(&name, false).is_err(), "a second process with the same name is refused");
        let (second, _, second_listening) = bind(&name, true).unwrap();
        assert_eq!(second, format!("{name}-2"), "unless it may take another name");
        drop(second_listening);
        let (found, path) = pick(Some(&name)).unwrap();
        assert_eq!(found, name);
        assert_eq!(session_for(Path::new("/srv/aqx.toml")).unwrap(), Some(path.clone()));
        assert_eq!(session_for(Path::new("/srv/other.toml")).unwrap(), None);
        assert_eq!(bots(&path).unwrap(), ["main"]);

        let bots = snapshot(&path).unwrap();
        assert_eq!(bots[0].bot, "main");

        let mut conn = Conn::open(&path).unwrap();
        conn.send(&json!({ "op": "logs", "bot": "main" })).unwrap();
        let line = conn.recv().unwrap();
        assert_eq!((line["type"].as_str(), line["line"]["text"].as_str()), (Some("log"), Some("hello")));
        assert_eq!(conn.recv().unwrap()["type"], "end");

        let ctl = |request: Value| Conn::ask(&path, &request);
        super::ctl(&path, "main", &Command::Pause).unwrap();
        assert!(cell.board().paused);
        ctl(json!({ "op": "ctl", "bot": "main", "cmd": "run", "script": "/x.ts", "args": [["n", "3"]] })).unwrap();
        assert!(matches!(cell.take_command(), Some(Command::Run { script, args }) if script == Path::new("/x.ts") && args == [("n".to_string(), "3".to_string())]));
        assert!(ctl(json!({ "op": "ctl", "bot": "nobody", "cmd": "stop" })).unwrap_err().contains("no bot named nobody"));
        drop(listening);
        assert!(!dir().join(format!("{name}.sock")).exists());
    }
}
