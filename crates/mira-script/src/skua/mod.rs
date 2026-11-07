pub mod protocol;
mod state;
use crate::{Cancel, Ended, Level, LoadError, Outcome, Output, Relogged, Start, restore};
use mira_client::{ActionId, Direction, Handle, Phase, Update};
use protocol::{Call, Message, RequestId, Worker};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};
const STOP_BUDGET: Duration = Duration::from_secs(10);
pub struct SkuaScript {
    path: PathBuf,
    output: Box<dyn FnMut(Output)>,
    cancel: Cancel,
}
pub fn worker() -> Result<PathBuf, LoadError> {
    find_worker(&mut || eprintln!("mira: building the Skua worker"))
}

fn find_worker(building: &mut dyn FnMut()) -> Result<PathBuf, LoadError> {
    if let Some(path) = std::env::var_os("MIRA_SKUA_WORKER") {
        return Ok(path.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.with_file_name("mira-skua");
        if path.is_file() {
            return Ok(path);
        }
    }
    development_worker(building)
}

fn development_worker(building: &mut dyn FnMut()) -> Result<PathBuf, LoadError> {
    let skua = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skua");
    build_worker(&skua, "dotnet".as_ref(), building)
}

fn build_worker(
    skua: &Path,
    dotnet: &std::ffi::OsStr,
    building: &mut dyn FnMut(),
) -> Result<PathBuf, LoadError> {
    let artifacts = skua.join("Host/artifacts");
    let worker = artifacts.join("publish/mira-skua");
    let stamp = artifacts.join("publish/.built");
    let fresh = || {
        let built = std::fs::metadata(&stamp).and_then(|m| m.modified()).ok();
        worker.is_file()
            && built.is_some_and(|built| newest_source(skua).is_none_or(|source| source <= built))
    };
    if fresh() {
        return Ok(worker);
    }
    let locked = |e: io::Error| LoadError(format!("couldn't lock the Skua worker build: {e}"));
    std::fs::create_dir_all(&artifacts).map_err(locked)?;
    let lock = std::fs::File::create(artifacts.join(".build.lock")).map_err(locked)?;
    lock.lock().map_err(locked)?;
    if fresh() {
        return Ok(worker);
    }
    building();
    let published = Command::new(dotnet)
        .arg("publish")
        .arg(skua.join("Host/Host.csproj"))
        .args(["-c", "Release", "--artifacts-path"])
        .arg(&artifacts)
        .arg("-o")
        .arg(artifacts.join("publish"))
        .output()
        .map_err(|e| {
            LoadError(if e.kind() == io::ErrorKind::NotFound {
                "couldn't find dotnet to build the Skua worker. Install the .NET SDK, or set MIRA_SKUA_WORKER to a built mira-skua".into()
            } else {
                format!("couldn't run dotnet to build the Skua worker: {e}")
            })
        })?;
    if !published.status.success() {
        return Err(LoadError(format!(
            "couldn't build the Skua worker:\n{}{}",
            String::from_utf8_lossy(&published.stdout),
            String::from_utf8_lossy(&published.stderr)
        )));
    }
    std::fs::write(&stamp, "")
        .map_err(|e| LoadError(format!("couldn't record the Skua worker build: {e}")))?;
    Ok(worker)
}

fn newest_source(dir: &Path) -> Option<std::time::SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name();
            if matches!(name.to_str(), Some("bin" | "obj" | "artifacts" | ".git"))
                || (name == "Scripts" && dir.file_name().is_some_and(|d| d == "vendor"))
            {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if meta.is_dir() {
                newest_source(&path)
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("cs" | "csproj" | "props")
            ) {
                meta.modified().ok()
            } else {
                None
            }
        })
        .max()
}
fn command(mode: &str, path: &Path, building: &mut dyn FnMut()) -> Result<Command, LoadError> {
    let data = match std::env::var_os("MIRA_SKUA_DATA") {
        Some(path) => PathBuf::from(path),
        None => dirs::data_local_dir()
            .ok_or_else(|| LoadError("couldn't find the application data directory".into()))?
            .join("mira/skua"),
    };
    let mut command = Command::new(find_worker(building)?);
    command.arg(mode).arg(path).env("MIRA_SKUA_DATA", data);
    Ok(command)
}
impl SkuaScript {
    pub fn load(
        path: &Path,
        mut output: impl FnMut(Output) + 'static,
        cancel: &Cancel,
    ) -> Result<Self, LoadError> {
        let path = std::path::absolute(path).map_err(|e| LoadError(e.to_string()))?;
        let mut building = || {
            output(Output::Log {
                level: Level::Log,
                text: "building the Skua worker".into(),
            });
        };
        let checked = command("check", &path, &mut building)?
            .output()
            .map_err(|e| LoadError(format!("couldn't check the C# script: {e}")))?;
        if !checked.status.success() {
            return Err(LoadError(format!(
                "{}{}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            )));
        }
        Ok(Self {
            path,
            output: Box::new(output),
            cancel: cancel.clone(),
        })
    }
    pub fn parse_args(
        &self,
        given: &[(String, String)],
    ) -> Result<serde_json::Map<String, Value>, String> {
        Ok(given
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect())
    }
    pub fn run(mut self, start: Start) -> Ended {
        let before = start.session.snapshot().settings.clone();
        let original = start.session.clone();
        let mut session = start.session;
        let mut server = start.server;
        let result = self.drive(&mut session, &mut server, start.args, start.account);
        session.cancel_all();
        if !matches!(session.snapshot().phase, Phase::Closed { .. }) {
            restore(
                &session,
                if session.same_session(&original) {
                    before
                } else {
                    Default::default()
                },
            );
        }
        Ended {
            outcome: result.unwrap_or_else(|e| Outcome::Failed(e.to_string())),
            session,
            server,
        }
    }
    fn drive(
        &mut self,
        session: &mut Handle,
        server: &mut String,
        args: serde_json::Map<String, Value>,
        account: Arc<dyn crate::Account>,
    ) -> io::Result<Outcome> {
        let output = &mut self.output;
        let mut building = || {
            output(Output::Log {
                level: Level::Log,
                text: "building the Skua worker".into(),
            });
        };
        let mut command = command("serve", &self.path, &mut building).map_err(io::Error::other)?;
        for (key, value) in args {
            command.arg("--arg").arg(format!(
                "{key}={}",
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string())
            ));
        }
        let mut child = Process(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?,
        );
        let mut stdin = child.0.stdin.take().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let stderr = child.0.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let sender = tx.clone();
        let reader = thread::Builder::new()
            .name("mira-skua-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let message = line.and_then(|line| {
                        serde_json::from_str::<Worker>(&line).map_err(io::Error::other)
                    });
                    if sender.send(Wake::Worker(message)).is_err() {
                        return;
                    }
                }
                let _ = sender.send(Wake::Exited);
            })?;
        let sender = tx.clone();
        let errors = thread::Builder::new()
            .name("mira-skua-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    if sender
                        .send(Wake::Log(line.unwrap_or_else(|e| e.to_string())))
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let sender = tx.clone();
        self.cancel.on_cancel(move || {
            let _ = sender.send(Wake::Cancel);
        });
        let mut generation = 0;
        let forwarding = Forwarding(Arc::new(AtomicU64::new(0)));
        forward(session, generation, tx.clone(), forwarding.0.clone())?;
        let mut projection = state::Projection::new();
        for message in projection.changed(session.snapshot(), server, session.server_host()) {
            send(&mut stdin, &message)?;
        }
        let mut actions: HashMap<ActionId, RequestId> = HashMap::new();
        let mut listening = [false; 2];
        let mut stopping = None;
        if self.cancel.is_cancelled() {
            send(
                &mut stdin,
                &Message::Stop {
                    stop: protocol::Empty {},
                },
            )?;
            stopping = Some(Instant::now());
        }
        let mut outcome = None;
        let mut suppress_close = false;
        let mut login_info: Option<(String, String)> = None;
        let mut relogging: Option<(RequestId, Instant)> = None;
        loop {
            if relogging.is_some_and(|(_, deadline)| Instant::now() >= deadline) {
                let (reply, _) = relogging.take().unwrap();
                send(
                    &mut stdin,
                    &Message::Error {
                        reply,
                        error: protocol::Error {
                            kind: protocol::ErrorKind::Http,
                            message: "login timed out".into(),
                        },
                    },
                )?;
            }
            if stopping.is_some_and(|since: Instant| since.elapsed() >= STOP_BUDGET) {
                child.0.kill()?;
                break;
            }
            let wake = match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(wake) => wake,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if child.0.try_wait()?.is_some() {
                        break;
                    }
                    continue;
                }
                Err(_) => break,
            };
            match wake {
                Wake::Cancel if stopping.is_none() => {
                    send(
                        &mut stdin,
                        &Message::Stop {
                            stop: protocol::Empty {},
                        },
                    )?;
                    stopping = Some(Instant::now());
                }
                Wake::Cancel => {}
                Wake::Log(text) => (self.output)(Output::Log {
                    level: Level::Log,
                    text,
                }),
                Wake::Exited => break,
                Wake::Worker(Err(error)) => return Err(error),
                Wake::Worker(Ok(Worker::Done { done })) => {
                    outcome = Some(match done.outcome {
                        protocol::Outcome::Finished => Outcome::Finished,
                        protocol::Outcome::Stopped => Outcome::Stopped(done.detail),
                        protocol::Outcome::Failed => Outcome::Failed(
                            done.detail.unwrap_or_else(|| "the C# script failed".into()),
                        ),
                    });
                    stopping.get_or_insert_with(Instant::now);
                }
                Wake::Worker(Ok(Worker::Request(request))) => {
                    let id = request.id;
                    let pending = match request.call {
                        Call::Join { map, cell, pad } => Some(session.join_at(&map, &cell, &pad)),
                        Call::Jump {
                            cell,
                            pad,
                            client_only,
                        } => Some(if client_only {
                            session.move_to_cell_local(&cell, &pad)
                        } else {
                            session.move_to_cell(&cell, &pad)
                        }),
                        Call::Walk { x, y } => Some(session.walk(x, y)),
                        Call::Goto { player } => Some(session.goto(&player)),
                        Call::Rest => Some(session.request_rest()),
                        Call::RestPacket => Some(session.request_rest_packet()),
                        Call::ExitCombat => Some(session.exit_combat()),
                        Call::Attack { target } => Some(session.attack(target)),
                        Call::UseSkill { slot, target } => Some(session.use_skill(
                            &slot,
                            target.map(|target| match target {
                                protocol::Target::Monster(id) => mira_client::Aim::Monster(id),
                                protocol::Target::Player(id) => mira_client::Aim::Player(id),
                            }),
                        )),
                        Call::LoadQuests { ids } => {
                            session.load_quests(&ids);
                            None
                        }
                        Call::Accept { quest } => Some(session.accept_quest(quest)),
                        Call::Complete {
                            quest,
                            reward,
                            times,
                            special,
                        } => Some(session.complete_quest_special(quest, reward, times, special)),
                        Call::Abandon { quest } => Some(session.abandon_quest(quest)),
                        Call::Pickup { item } => Some(session.request_drop(item)),
                        Call::GetMapItem { item } => Some(session.get_map_item(item)),
                        Call::LoadCells => Some(session.load_cells()),
                        Call::LoadBank => Some(session.load_bank()),
                        Call::Deposit { item } => Some(session.deposit(item)),
                        Call::Withdraw { item } => Some(session.withdraw(item)),
                        Call::Swap { inventory, bank } => Some(session.swap(inventory, bank)),
                        Call::LoadShop { shop } => Some(session.load_shop(shop)),
                        Call::Buy { listing, quantity } => Some(session.buy(listing, quantity)),
                        Call::Sell { item, quantity } => Some(session.sell(item, quantity)),
                        Call::Equip { item } => Some(session.equip(item)),
                        Call::Unequip { item } => Some(session.unequip(item)),
                        Call::UseItem { item } => Some(session.use_item(item)),
                        Call::Send { text } => Some(session.send_raw(&text)),
                        Call::SetSkillLastUse { slot, at } => {
                            session.set_skill_last_use(&slot, projection.instant_at(at));
                            Some(session.ready())
                        }
                        Call::Untarget => {
                            session.untarget();
                            None
                        }
                        Call::Inject { text } => {
                            session.inject(&text);
                            Some(session.ready())
                        }
                        Call::SetAggro { mode } => {
                            session.set_aggro(match mode {
                                protocol::Aggro::Off => None,
                                protocol::Aggro::All => Some(mira_client::Aggro::All),
                                protocol::Aggro::Cell => Some(mira_client::Aggro::Cell),
                                protocol::Aggro::Only(ids) => {
                                    Some(mira_client::Aggro::Only(ids.into_iter().collect()))
                                }
                            });
                            None
                        }
                        Call::SetSpawnPoint { cell, pad } => {
                            session.set_spawn_point(&cell, &pad);
                            None
                        }
                        Call::SetCounterHold { mode } => {
                            session.set_counter_hold(match mode {
                                protocol::CounterHold::Off => mira_client::CounterHold::Off,
                                protocol::CounterHold::All => mira_client::CounterHold::All,
                                protocol::CounterHold::Attacks => mira_client::CounterHold::Attacks,
                            });
                            None
                        }
                        Call::Listen { direction, enabled } => {
                            listening[match direction {
                                protocol::Direction::In => 0,
                                protocol::Direction::Out => 1,
                            }] = enabled;
                            None
                        }
                        Call::Log { level, text } => {
                            (self.output)(Output::Log {
                                level: match level {
                                    protocol::Level::Log => Level::Log,
                                    protocol::Level::Warn => Level::Warn,
                                    protocol::Level::Error => Level::Error,
                                },
                                text,
                            });
                            None
                        }
                        Call::Status { text } => {
                            (self.output)(Output::Status(text));
                            None
                        }
                        Call::Servers => {
                            let account = account.clone();
                            let tx = tx.clone();
                            thread::spawn(move || {
                                let _ = tx.send(Wake::Servers(id, account.servers()));
                            });
                            continue;
                        }
                        Call::SetLoginInfo { username, password } => {
                            login_info = Some((username, password));
                            None
                        }
                        Call::Logout => {
                            suppress_close = false;
                            session.logout();
                            None
                        }
                        Call::CancelRelogin { request } => {
                            if relogging.is_some_and(|(pending, _)| pending == request) {
                                relogging = None;
                            }
                            None
                        }
                        Call::Relogin {
                            server: target,
                            timeout_ms,
                        } => {
                            suppress_close = true;
                            relogging =
                                Some((id, Instant::now() + Duration::from_millis(timeout_ms)));
                            let target = target.unwrap_or_else(|| server.clone());
                            let account = account.clone();
                            let tx = tx.clone();
                            let old = session.clone();
                            let login_info = login_info.clone();
                            thread::spawn(move || {
                                old.logout();
                                let result = match login_info {
                                    Some((user, pass)) => account.login_as(&target, &user, &pass),
                                    None => account.login(&target),
                                }
                                .and_then(|handle| {
                                    handle.ready().wait().map_err(|e| {
                                        handle.logout();
                                        e.to_string()
                                    })?;
                                    Ok(Relogged(Some((handle, target))))
                                });
                                let _ = tx.send(Wake::Relogged(id, result));
                            });
                            continue;
                        }
                    };
                    if let Some(pending) = pending {
                        actions.insert(pending.id(), id);
                    } else {
                        send(
                            &mut stdin,
                            &Message::Ok {
                                reply: id,
                                ok: Value::Null,
                            },
                        )?;
                    }
                }
                Wake::Update(g, update) if g == generation => {
                    for message in
                        projection.changed(session.snapshot(), server, session.server_host())
                    {
                        send(&mut stdin, &message)?;
                    }
                    match update {
                        Update::Event(mira_client::Event::Phase(mira_client::Phase::Closed {
                            reason,
                        })) if !suppress_close && relogging.is_none() && stopping.is_none() => {
                            send(
                                &mut stdin,
                                &Message::Disconnected {
                                    disconnected: matches!(
                                        reason,
                                        mira_client::Disconnect::Kicked { .. }
                                    ),
                                },
                            )?;
                        }
                        Update::Event(event) => send(
                            &mut stdin,
                            &Message::Event {
                                event: serde_json::to_value(event)?,
                            },
                        )?,
                        Update::Packet(direction, text)
                            if listening[if direction == Direction::In { 0 } else { 1 }] =>
                        {
                            let direction = if direction == Direction::In {
                                protocol::Direction::In
                            } else {
                                protocol::Direction::Out
                            };
                            send(
                                &mut stdin,
                                &Message::Packet {
                                    packet: protocol::Packet { direction, text },
                                },
                            )?;
                        }
                        Update::Done { id, result } => {
                            if let Some(reply) = actions.remove(&id) {
                                let message = match result {
                                    Ok(()) => Message::Ok {
                                        reply,
                                        ok: Value::Null,
                                    },
                                    Err(error) => Message::Error {
                                        reply,
                                        error: error.into(),
                                    },
                                };
                                send(&mut stdin, &message)?;
                            }
                        }
                        _ => {}
                    }
                }
                Wake::Update(..) => {}
                Wake::Servers(reply, result) => {
                    let message = match result {
                        Ok(servers) => {
                            let servers: Vec<_> = servers.iter().map(|s| json!({
                                "name": s.name, "host": s.host, "port": s.port, "online": s.online,
                                "members_only": s.members_only, "players": s.players, "capacity": s.capacity
                            })).collect();
                            Message::Ok {
                                reply,
                                ok: json!(servers),
                            }
                        }
                        Err(message) => Message::Error {
                            reply,
                            error: protocol::Error {
                                kind: protocol::ErrorKind::Http,
                                message,
                            },
                        },
                    };
                    send(&mut stdin, &message)?;
                }
                Wake::Relogged(reply, result) => {
                    if !relogging.is_some_and(|(id, _)| id == reply) {
                        continue;
                    }
                    relogging = None;
                    match result {
                        Ok(relogged) => {
                            for (_, reply) in actions.drain() {
                                send(
                                    &mut stdin,
                                    &Message::Error {
                                        reply,
                                        error: mira_client::ActionError::Cancelled.into(),
                                    },
                                )?;
                            }
                            let (next, name) = relogged.take();
                            *session = next;
                            suppress_close = false;
                            *server = name;
                            generation += 1;
                            send(
                                &mut stdin,
                                &Message::Reset {
                                    reset: protocol::Empty {},
                                },
                            )?;
                            projection.reset();
                            forwarding.0.store(generation, Ordering::Release);
                            forward(session, generation, tx.clone(), forwarding.0.clone())?;
                            for message in projection.changed(
                                session.snapshot(),
                                server,
                                session.server_host(),
                            ) {
                                send(&mut stdin, &message)?;
                            }
                            send(
                                &mut stdin,
                                &Message::Ok {
                                    reply,
                                    ok: Value::Null,
                                },
                            )?;
                        }
                        Err(message) => send(
                            &mut stdin,
                            &Message::Error {
                                reply,
                                error: protocol::Error {
                                    kind: protocol::ErrorKind::Http,
                                    message,
                                },
                            },
                        )?,
                    }
                }
            }
        }
        if child.0.try_wait()?.is_none() {
            child.0.kill()?;
        }
        let status = child.0.wait()?;
        let _ = reader.join();
        let _ = errors.join();
        while let Ok(wake) = rx.try_recv() {
            if let Wake::Log(text) = wake {
                (self.output)(Output::Log {
                    level: Level::Log,
                    text,
                });
            }
        }
        Ok(if self.cancel.is_cancelled() {
            Outcome::Cancelled
        } else {
            outcome
                .unwrap_or_else(|| Outcome::Failed(format!("the C# worker exited with {status}")))
        })
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn send(writer: &mut impl Write, message: &Message) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
enum Wake {
    Worker(io::Result<Worker>),
    Exited,
    Log(String),
    Cancel,
    Update(u64, Update),
    Servers(RequestId, Result<Vec<mira_client::Server>, String>),
    Relogged(RequestId, Result<Relogged, String>),
}
struct Forwarding(Arc<AtomicU64>);
impl Drop for Forwarding {
    fn drop(&mut self) {
        self.0.store(u64::MAX, Ordering::Release);
    }
}
fn forward(
    session: &Handle,
    generation: u64,
    tx: mpsc::Sender<Wake>,
    current: Arc<AtomicU64>,
) -> io::Result<()> {
    let updates = session.updates();
    thread::Builder::new()
        .name("mira-skua-updates".into())
        .spawn(move || {
            while current.load(Ordering::Acquire) == generation {
                match updates.recv_timeout(Duration::from_millis(100)) {
                    Ok(update) => {
                        if tx.send(Wake::Update(generation, update)).is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn accounts_starting_together_build_a_stale_worker_once() {
        let root = std::env::temp_dir().join(format!("mira-worker-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let skua = root.join("skua");
        std::fs::create_dir_all(skua.join("Host")).unwrap();
        std::fs::write(skua.join("Host/Host.csproj"), "").unwrap();
        let builds = root.join("builds");
        let dotnet = root.join("dotnet");
        std::fs::write(
            &dotnet,
            format!(
                "#!/bin/sh\necho >> '{}'\nsleep 0.3\nwhile [ \"$1\" != -o ]; do shift; done\nmkdir -p \"$2\" && touch \"$2/mira-skua\"\n",
                builds.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&dotnet, std::fs::Permissions::from_mode(0o755)).unwrap();
        let starts: Vec<_> = (0..3)
            .map(|_| {
                let (skua, dotnet) = (skua.clone(), dotnet.clone());
                thread::spawn(move || build_worker(&skua, dotnet.as_os_str(), &mut || {}))
            })
            .collect();
        for start in starts {
            start.join().unwrap().unwrap();
        }
        let built = std::fs::read_to_string(&builds).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(built.lines().count(), 1);
    }
}
