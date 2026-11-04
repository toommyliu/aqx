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

pub const PROTOCOL: u32 = 9;
const BUILD: &str = env!("AQX_BUILD");
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
        account: Option<String>,
        #[serde(default)]
        follow: bool,
    },
    Ctl {
        account: String,
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
            Ok(Request::Info) => json!({ "type": "info", "protocol": PROTOCOL, "build": BUILD, "name": hub.name, "config": config, "accounts": names(hub) }),
            Ok(Request::Snapshot) => json!({ "type": "snapshot", "protocol": PROTOCOL, "name": hub.name, "accounts": snapshots(hub) }),
            Ok(Request::Ctl { account, command }) => match hub.send(&account, command) {
                Ok(changed) => json!({ "type": "ok", "changed": changed }),
                Err(e) => json!({ "type": "error", "message": e }),
            },
            Ok(Request::Subscribe) => return subscribe(hub, &mut out),
            Ok(Request::Logs { account, follow }) => return logs(hub, account.as_deref(), follow, &mut out),
        };
        if !send(&mut out, &reply) {
            return;
        }
    }
}

fn snapshots(hub: &Arc<Hub>) -> Vec<Snapshot> {
    let fresh = hub.cells.all().iter().all(|c| c.board().snapshot.is_some()) && hub.watched();
    if !fresh {
        let _watching = hub.watch();
        thread::sleep(FRESH);
    }
    hub.cells.all().iter().map(|c| c.board().snapshot.clone().unwrap_or_else(|| Snapshot { account: c.name.clone(), phase: "STARTING".into(), ..Snapshot::default() })).collect()
}

fn names(hub: &Hub) -> Vec<String> {
    hub.cells.all().iter().map(|c| c.name.clone()).collect()
}

fn subscribe(hub: &Arc<Hub>, out: &mut UnixStream) {
    let _watching = hub.watch();
    let hello = names(hub);
    if !send(out, &json!({ "type": "hello", "protocol": PROTOCOL, "name": hub.name, "accounts": hello })) {
        return;
    }
    let mut shown: Vec<Option<Snapshot>> = vec![None; hello.len()];
    let mut next: Vec<u64> = vec![0; hello.len()];
    loop {
        let ended = hub.ended();
        let cells = hub.cells.all();
        for cell in &cells[shown.len()..] {
            if !send(out, &json!({ "type": "account", "account": cell.name })) {
                return;
            }
        }
        shown.resize(cells.len(), None);
        next.resize(cells.len(), 0);
        for (i, cell) in cells.iter().enumerate() {
            let (snapshot, lines) = {
                let board = cell.board();
                let lines: Vec<LogLine> = board.log.iter().filter(|l| l.seq >= next[i]).cloned().collect();
                (board.snapshot.clone(), lines)
            };
            for line in lines {
                next[i] = line.seq + 1;
                if !send(out, &json!({ "type": "log", "account": cell.name, "line": line })) {
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
        if ended {
            return;
        }
        thread::sleep(TICK);
    }
}

fn logs(hub: &Arc<Hub>, account: Option<&str>, follow: bool, out: &mut UnixStream) {
    let cells: Vec<_> = hub.cells.all().into_iter().filter(|c| account.is_none_or(|name| c.name == name)).collect();
    if cells.is_empty() {
        send(out, &json!({ "type": "error", "message": format!("no account named {} in {}", account.unwrap_or_default(), hub.name) }));
        return;
    }
    let mut next = vec![0u64; cells.len()];
    loop {
        let ended = hub.ended();
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
            if !send(out, &json!({ "type": "log", "account": name, "line": line })) {
                return;
            }
        }
        if !follow || ended {
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

#[derive(Deserialize)]
struct Resumable {
    #[serde(alias = "bot")]
    account: String,
    phase: String,
    run: String,
    script: Option<String>,
    #[serde(default)]
    args: Vec<(String, String)>,
}

pub fn to_resume(path: &Path) -> Result<Vec<(String, Command)>, String> {
    resumable(&Conn::ask(path, &json!({ "op": "snapshot" }))?)
}

fn resumable(reply: &Value) -> Result<Vec<(String, Command)>, String> {
    let listed = reply.get("accounts").or_else(|| reply.get("bots")).cloned().unwrap_or_default();
    let accounts: Vec<Resumable> = serde_json::from_value(listed).map_err(|e| format!("unreadable snapshot: {e}"))?;
    Ok(accounts
        .into_iter()
        .filter(|a| a.phase != "OFFLINE")
        .map(|a| match (a.run.as_str(), a.script) {
            ("running", Some(script)) => (a.account, Command::Run { script: script.into(), args: a.args }),
            _ => (a.account, Command::Login),
        })
        .collect())
}

pub fn accounts(path: &Path) -> Result<Vec<String>, String> {
    let info = Conn::ask(path, &json!({ "op": "info" }))?;
    serde_json::from_value(info["accounts"].clone()).map_err(|e| e.to_string())
}

pub fn same_build(reply: &Value) -> Result<(), String> {
    if reply["protocol"] == PROTOCOL {
        return Ok(());
    }
    let name = reply["name"].as_str().unwrap_or("the session");
    Err(format!("{name} was started by a different build of aqx; run aqx down and start it again"))
}

pub fn info(path: &Path) -> Result<Value, String> {
    Conn::ask(path, &json!({ "op": "info" }))
}

pub fn other_build(info: &Value) -> bool {
    info["protocol"] != PROTOCOL || info["build"] != BUILD
}

pub fn snapshot(path: &Path) -> Result<Vec<Snapshot>, String> {
    let reply = Conn::ask(path, &json!({ "op": "snapshot" }))?;
    same_build(&reply)?;
    serde_json::from_value(reply["accounts"].clone()).map_err(|e| e.to_string())
}

pub fn ctl(path: &Path, account: &str, command: &Command) -> Result<bool, String> {
    let mut request = serde_json::to_value(command).map_err(|e| e.to_string())?;
    request["op"] = json!("ctl");
    request["account"] = json!(account);
    Conn::ask(path, &request).map(|reply| reply["changed"].as_bool().unwrap_or(true))
}

pub fn find_account(account: &str) -> Result<PathBuf, String> {
    let all = processes()?;
    if all.is_empty() {
        return Err("no aqx process is running".into());
    }
    for (_, path) in &all {
        if accounts(path).is_ok_and(|accounts| accounts.iter().any(|a| a == account)) {
            return Ok(path.clone());
        }
    }
    Err(format!("no running aqx process has an account named {account}"))
}

#[cfg(test)]
mod tests {
    use aqx_core::Control;

    use super::*;
    use crate::hub::{AccountCell, Print};

    fn keys(value: &Value, at: &str, out: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (key, inner) in fields {
                    let path = if at.is_empty() { key.clone() } else { format!("{at}.{key}") };
                    keys(inner, &path, out);
                    out.push(path);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| keys(item, at, out)),
            _ => {}
        }
    }

    #[test]
    fn an_ending_session_refuses_commands() {
        let control = Control::new().unwrap();
        let hub = Hub::new("ending", vec![Arc::new(AccountCell::local("main", control.stop_handle()))], Print::Quiet);
        assert!(hub.end_if_idle());
        assert_eq!(hub.send("main", Command::Login), Err("ending is ending".to_string()));
    }

    #[test]
    fn a_session_from_another_build_is_named_with_the_way_out() {
        assert_eq!(same_build(&json!({ "type": "info", "protocol": PROTOCOL, "name": "farm" })), Ok(()));
        let before_protocols = json!({ "type": "info", "name": "farm", "accounts": ["hero"] });
        assert_eq!(same_build(&before_protocols), Err("farm was started by a different build of aqx; run aqx down and start it again".into()));
    }

    #[test]
    fn a_session_is_another_build_when_its_sources_or_protocol_differ() {
        assert!(!other_build(&json!({ "protocol": PROTOCOL, "build": BUILD })));
        assert!(other_build(&json!({ "protocol": PROTOCOL, "build": "0000000000000000" })));
        assert!(other_build(&json!({ "protocol": PROTOCOL - 1, "build": BUILD })));
        assert!(other_build(&json!({ "protocol": PROTOCOL })), "a session from before builds were reported");
    }

    #[test]
    fn a_restart_resumes_online_accounts_and_their_scripts() {
        let controls: Vec<Control> = (0..3).map(|_| Control::new().unwrap()).collect();
        let cells: Vec<Arc<AccountCell>> = ["farmer", "idler", "away"].iter().zip(&controls).map(|(name, c)| Arc::new(AccountCell::local(name, c.stop_handle()))).collect();
        let show = |cell: &AccountCell, phase: &str, run: &str, script: Option<&str>| {
            let args = vec![("rounds".to_string(), "3".to_string())];
            cell.board().snapshot = Some(Snapshot { account: cell.name.clone(), phase: phase.into(), run: run.into(), script: script.map(Into::into), args, ..Snapshot::default() });
        };
        show(&cells[0], "READY", "running", Some("/srv/farm.ts"));
        show(&cells[1], "READY", "done", Some("/srv/farm.ts"));
        show(&cells[2], "OFFLINE", "running", Some("/srv/farm.ts"));
        let name = format!("resume-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, cells, Print::Quiet));
        let (_, listener, _listening) = bind(&name, false).unwrap();
        serve(listener, hub, None).unwrap();
        let resume = to_resume(&pick(Some(&name)).unwrap().1).unwrap();
        let shown: Vec<String> = resume.iter().map(|(account, command)| format!("{account} {}", serde_json::to_string(command).unwrap())).collect();
        assert_eq!(shown, [r#"farmer {"cmd":"run","script":"/srv/farm.ts","args":[["rounds","3"]]}"#, r#"idler {"cmd":"login"}"#]);
    }

    #[test]
    fn a_restart_reads_a_session_from_before_accounts_were_named() {
        let protocol_2 = json!({ "type": "snapshot", "protocol": 2, "name": "aqx", "bots": [
            { "bot": "farmer", "phase": "READY", "run": "running", "script": "/srv/farm.ts", "args": [["rounds", "3"]] },
            { "bot": "idle", "phase": "READY", "run": "done", "script": null, "args": [] },
        ] });
        let shown: Vec<String> =
            resumable(&protocol_2).unwrap().iter().map(|(account, command)| format!("{account} {}", serde_json::to_string(command).unwrap())).collect();
        assert_eq!(shown, [r#"farmer {"cmd":"run","script":"/srv/farm.ts","args":[["rounds","3"]]}"#, r#"idle {"cmd":"login"}"#]);
    }

    #[test]
    fn the_wire_format_matches_its_protocol_number() {
        use crate::hub::{Aura, Bar, Game, Player, QuestProgress, SkillState, Snapshot, Stack, Target};
        let game = Game {
            skills: vec![SkillState::default()],
            auras: vec![Aura::default()],
            target: Some(Target::default()),
            cell_players: vec![Player::default()],
            drops: vec![Stack::default()],
            quests: vec![QuestProgress::default()],
            temp: vec![Stack::default()],
            inventory: vec![Stack::default()],
            bank: Some(vec![Stack::default()]),
            ..Game::default()
        };
        let snapshot = Snapshot {
            script: Some(String::new()),
            progress: vec![Bar { eta_secs: Some(0), ..Bar::default() }],
            gold_gained: Some(0),
            game: Some(game),
            ..Snapshot::default()
        };
        let line = LogLine { seq: 0, t: 0, source: String::new(), text: String::new() };
        let mut shape = Vec::new();
        keys(&json!({ "snapshot": snapshot, "log": line }), "", &mut shape);
        shape.sort();
        let pinned = "log log.seq log.source log.t log.text snapshot snapshot.account snapshot.args snapshot.deaths \
             snapshot.detail snapshot.drops snapshot.game snapshot.game.afk snapshot.game.area \
             snapshot.game.auras snapshot.game.auras.name snapshot.game.auras.stacks snapshot.game.bag_slots \
             snapshot.game.bag_used snapshot.game.bank snapshot.game.bank.equipped snapshot.game.bank.max \
             snapshot.game.bank.name snapshot.game.bank.qty snapshot.game.bank.rank snapshot.game.bank_slots \
             snapshot.game.bank_used snapshot.game.cell snapshot.game.cell_players \
             snapshot.game.cell_players.level snapshot.game.cell_players.name snapshot.game.class \
             snapshot.game.class_rank snapshot.game.dead snapshot.game.drops snapshot.game.drops.equipped \
             snapshot.game.drops.max snapshot.game.drops.name snapshot.game.drops.qty snapshot.game.drops.rank snapshot.game.gold \
             snapshot.game.hp snapshot.game.inventory snapshot.game.inventory.equipped \
             snapshot.game.inventory.max snapshot.game.inventory.name snapshot.game.inventory.qty \
             snapshot.game.inventory.rank snapshot.game.level snapshot.game.max_hp snapshot.game.max_mp \
             snapshot.game.misc_slots snapshot.game.misc_used snapshot.game.mp snapshot.game.pad \
             snapshot.game.quests snapshot.game.quests.have snapshot.game.quests.name snapshot.game.quests.need \
             snapshot.game.skills snapshot.game.skills.name snapshot.game.skills.ready_pct snapshot.game.target \
             snapshot.game.target.hp snapshot.game.target.max_hp snapshot.game.target.name snapshot.game.temp \
             snapshot.game.temp.equipped snapshot.game.temp.max snapshot.game.temp.name snapshot.game.temp.qty \
             snapshot.game.temp.rank snapshot.gold_gained snapshot.kills snapshot.paused snapshot.phase \
             snapshot.progress snapshot.progress.current snapshot.progress.eta_secs snapshot.progress.label \
             snapshot.progress.total snapshot.run snapshot.script snapshot.server snapshot.stalled \
             snapshot.status snapshot.up_secs snapshot.user";
        assert_eq!((PROTOCOL, shape.join(" ").as_str()), (9, pinned), "the wire format changed: bump PROTOCOL, then update this shape");
    }

    #[test]
    fn a_subscriber_hears_about_accounts_added_after_it_subscribed() {
        let control = Control::new().unwrap();
        let name = format!("test-added-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, vec![Arc::new(AccountCell::local("main", control.stop_handle()))], Print::Quiet));
        let (_, listener, _listening) = bind(&name, false).unwrap();
        serve(listener, hub.clone(), None).unwrap();
        let mut conn = Conn::open(&pick(Some(&name)).unwrap().1).unwrap();
        conn.writer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        conn.send(&json!({ "op": "subscribe" })).unwrap();
        assert_eq!(conn.recv().unwrap()["accounts"], json!(["main"]));

        hub.cells.add(Arc::new(AccountCell::local("alt", control.stop_handle())));
        let added = std::iter::from_fn(|| conn.recv()).find(|m| m["type"] == "account").expect("no account message within 5s");
        assert_eq!(added["account"], "alt");
    }

    #[test]
    fn a_subscriber_hears_the_last_lines_before_the_session_ends() {
        let control = Control::new().unwrap();
        let cell = Arc::new(AccountCell::local("main", control.stop_handle()));
        let name = format!("test-ends-{}", std::process::id());
        let hub = Arc::new(Hub::new(&name, vec![cell.clone()], Print::Quiet));
        cell.board().online = true;
        let (_, listener, _listening) = bind(&name, false).unwrap();
        serve(listener, hub.clone(), None).unwrap();
        let mut conn = Conn::open(&pick(Some(&name)).unwrap().1).unwrap();
        conn.writer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        conn.send(&json!({ "op": "subscribe" })).unwrap();
        conn.recv().unwrap();
        while !hub.watched() {
            thread::sleep(Duration::from_millis(5));
        }

        hub.say(&cell, "aqx", "logged out");
        cell.board().online = false;
        assert!(hub.end_if_idle());
        let started = std::time::Instant::now();
        hub.let_watchers_finish(Duration::from_secs(2));
        assert!(started.elapsed() < Duration::from_secs(1), "the subscriber hangs on after the session ends");
        let heard: Vec<Value> = std::iter::from_fn(|| conn.recv()).collect();
        assert!(heard.iter().any(|m| m["line"]["text"] == "logged out"), "{heard:?}");
    }

    #[test]
    fn answers_snapshots_logs_and_commands() {
        let control = Control::new().unwrap();
        let cell = Arc::new(AccountCell::local("main", control.stop_handle()));
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
        assert_eq!(accounts(&path).unwrap(), ["main"]);

        let accounts = snapshot(&path).unwrap();
        assert_eq!(accounts[0].account, "main");

        let mut conn = Conn::open(&path).unwrap();
        conn.send(&json!({ "op": "logs", "account": "main" })).unwrap();
        let line = conn.recv().unwrap();
        assert_eq!((line["type"].as_str(), line["line"]["text"].as_str()), (Some("log"), Some("hello")));
        assert_eq!(conn.recv().unwrap()["type"], "end");

        let ctl = |request: Value| Conn::ask(&path, &request);
        super::ctl(&path, "main", &Command::Pause).unwrap();
        assert_eq!(super::ctl(&path, "main", &Command::Login), Ok(false), "an online account is already logged in");
        assert!(cell.board().paused);
        ctl(json!({ "op": "ctl", "account": "main", "cmd": "run", "script": "/x.ts", "args": [["n", "3"]] })).unwrap();
        assert!(matches!(cell.take_command(), Some(Command::Run { script, args }) if script == Path::new("/x.ts") && args == [("n".to_string(), "3".to_string())]));
        assert!(ctl(json!({ "op": "ctl", "account": "nobody", "cmd": "stop" })).unwrap_err().contains("no account named nobody"));
        drop(listening);
        assert!(!dir().join(format!("{name}.sock")).exists());
    }
}
