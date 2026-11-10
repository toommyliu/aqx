pub mod library;
pub mod protocol;
mod state;
use crate::{Cancel, Ended, Level, LoadError, Outcome, Output, Relogged, Start, restore_after_run};
use aqx_client::{ActionId, Direction, Handle, Pending, Phase, Update};
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
const AWAIT_ONLINE: Duration = Duration::from_secs(365 * 24 * 60 * 60);
pub struct SkuaScript {
    path: PathBuf,
    skua: PathBuf,
    output: Box<dyn FnMut(Output)>,
    cancel: Cancel,
}
pub fn worker(building: &mut dyn FnMut()) -> Result<PathBuf, LoadError> {
    if let Some(path) = std::env::var_os("AQX_SKUA_WORKER") {
        return Ok(path.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.with_file_name("aqx-skua");
        if path.is_file() {
            return Ok(path);
        }
    }
    development_worker(building)
}

fn development_worker(building: &mut dyn FnMut()) -> Result<PathBuf, LoadError> {
    let clone = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("aqx-script sits two folders inside the clone");
    let skua = clone.join("skua");
    if !skua.join("Host/Host.csproj").is_file() {
        return Err(LoadError(format!(
            "aqx builds the Skua worker from the clone it was installed from, and {} isn't there anymore. Install aqx again from a clone with `cargo install --path crates/aqx`",
            clone.display()
        )));
    }
    if !skua.join("vendor/Skua/Skua.Core").is_dir() {
        return Err(LoadError(format!(
            "the Skua worker needs the Skua submodule. Run `git submodule update --init skua/vendor/Skua` in {}",
            clone.display()
        )));
    }
    build_worker(&skua, "dotnet".as_ref(), building)
}

fn build_worker(
    skua: &Path,
    dotnet: &std::ffi::OsStr,
    building: &mut dyn FnMut(),
) -> Result<PathBuf, LoadError> {
    let artifacts = skua.join("Host/artifacts");
    let worker = artifacts.join("publish/aqx-skua");
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
                "couldn't find dotnet to build the Skua worker. Install the .NET SDK, or set AQX_SKUA_WORKER to a built aqx-skua".into()
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Settings {
    pub storage: Option<String>,
    pub options: Vec<Setting>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Setting {
    pub group: String,
    pub name: String,
    pub label: String,
    pub description: String,
    pub kind: Kind,
    pub choices: Vec<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bool,
    Number,
    Text,
    Choice,
}

pub fn script_options(
    path: &Path,
    skua: &Path,
    changes: &[(String, String)],
) -> Result<Settings, LoadError> {
    let path = std::path::absolute(path).map_err(|e| LoadError(e.to_string()))?;
    settings("options", path.as_os_str(), skua, changes)
}

pub fn core_bots_options(
    user: &str,
    skua: &Path,
    changes: &[(String, String)],
) -> Result<Settings, LoadError> {
    settings("cbo", user.as_ref(), skua, changes)
}

fn settings(
    mode: &str,
    arg: &std::ffi::OsStr,
    skua: &Path,
    changes: &[(String, String)],
) -> Result<Settings, LoadError> {
    let mut command = command(mode, arg, skua, &mut || {})?;
    for (key, value) in changes {
        command.arg("--set").arg(format!("{key}={value}"));
    }
    let out = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| LoadError(format!("couldn't run the Skua worker: {e}")))?;
    if !out.status.success() {
        return Err(failed(
            skua,
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| LoadError(format!("the Skua worker sent {e}")))
}

fn command(
    mode: &str,
    arg: &std::ffi::OsStr,
    skua: &Path,
    building: &mut dyn FnMut(),
) -> Result<Command, LoadError> {
    let mut command = Command::new(worker(building)?);
    command.arg(mode).arg(arg).env("AQX_SKUA_DATA", skua);
    Ok(command)
}

fn failed(skua: &Path, why: String) -> LoadError {
    if library::scripts(skua).is_dir() {
        return LoadError(why);
    }
    LoadError(format!(
        "{}\nThe Skua scripts aren't downloaded. Run `aqx skua update`, or press Update on the Skua tab of Run in the web UI",
        why.trim_end()
    ))
}
impl SkuaScript {
    pub fn load(
        path: &Path,
        skua: &Path,
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
        let checked = command("check", path.as_os_str(), skua, &mut building)?
            .output()
            .map_err(|e| LoadError(format!("couldn't check the C# script: {e}")))?;
        if !checked.status.success() {
            return Err(failed(
                skua,
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&checked.stdout),
                    String::from_utf8_lossy(&checked.stderr)
                ),
            ));
        }
        Ok(Self {
            path,
            skua: skua.to_path_buf(),
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
            restore_after_run(
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
        let mut command = command("serve", self.path.as_os_str(), &self.skua, &mut building)
            .map_err(io::Error::other)?;
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
            .name("aqx-skua-reader".into())
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
            .name("aqx-skua-stderr".into())
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
        let mut actions: HashMap<ActionId, (RequestId, Pending)> = HashMap::new();
        let mut ended = false;
        let mut offline = false;
        let mut awaiting_online = None;
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
                            auto_correct,
                        } => Some(session.jump(
                            &cell,
                            &pad,
                            aqx_client::Jump {
                                client_only,
                                correct_pad: auto_correct,
                            },
                        )),
                        Call::Walk { x, y } => Some(session.walk(x, y)),
                        Call::Goto { player } => Some(session.goto(&player)),
                        Call::Rest => Some(session.request_rest()),
                        Call::RestPacket => Some(session.request_rest_packet()),
                        Call::ExitCombat => Some(session.exit_combat()),
                        Call::Attack { target } => Some(session.attack(target)),
                        Call::UseSkill { slot, target } => Some(session.fire_skill(
                            &slot,
                            target.map(|target| match target {
                                protocol::Target::Monster(id) => aqx_client::Aim::Monster(id),
                                protocol::Target::Player(id) => aqx_client::Aim::Player(id),
                            }),
                        )),
                        Call::LoadQuests { ids } => {
                            session.load_quests(&ids);
                            None
                        }
                        Call::Accept { quest } => Some(session.accept_quest_unchecked(quest)),
                        Call::Complete {
                            quest,
                            reward,
                            times,
                            special,
                        } => Some(session.complete_quest_unchecked(quest, reward, times, special)),
                        Call::Abandon { quest } => Some(session.abandon_quest(quest)),
                        Call::ForgetQuests => Some(session.forget_quests()),
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
                        Call::EquipUnchecked { item } => Some(session.equip_unchecked(item)),
                        Call::Unequip { item } => Some(session.unequip(item)),
                        Call::UseItem { item } => Some(session.use_item(item)),
                        Call::Send { text } => Some(session.send_now(&text)),
                        Call::SetSkillLastUse { slot, at } => {
                            session.set_skill_last_use(&slot, projection.instant_at(at));
                            Some(session.sync())
                        }
                        Call::Untarget => {
                            session.untarget();
                            None
                        }
                        Call::Inject { text } => {
                            session.inject(&text);
                            Some(session.sync())
                        }
                        Call::SetAggro { mode } => {
                            session.set_aggro(match mode {
                                protocol::Aggro::Off => None,
                                protocol::Aggro::All => Some(aqx_client::Aggro::All),
                                protocol::Aggro::Cell => Some(aqx_client::Aggro::Cell),
                                protocol::Aggro::Only(ids) => {
                                    Some(aqx_client::Aggro::Only(ids.into_iter().collect()))
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
                                protocol::CounterHold::Off => aqx_client::CounterHold::Off,
                                protocol::CounterHold::All => aqx_client::CounterHold::All,
                                protocol::CounterHold::Attacks => aqx_client::CounterHold::Attacks,
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
                        Call::SetAutoRelogin { on } => {
                            session.set_auto_relogin(on);
                            None
                        }
                        Call::AwaitOnline => {
                            relogging = Some((id, Instant::now() + AWAIT_ONLINE));
                            if let Phase::Closed { reason } = &session.snapshot().phase {
                                let _ = tx.send(Wake::Online(id, Err(stayed_offline(reason))));
                            } else if offline {
                                awaiting_online = Some(id);
                            } else {
                                let _ = tx.send(Wake::Online(id, Ok(())));
                            }
                            continue;
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
                    match pending {
                        Some(pending) if ended => send(&mut stdin, &reply(id, pending.wait()))?,
                        Some(pending) => {
                            actions.insert(pending.id(), (id, pending));
                        }
                        None => send(&mut stdin, &reply(id, Ok(())))?,
                    }
                }
                Wake::Update(g, update) if g == generation => {
                    if let Update::State = update {
                        for message in
                            projection.changed(session.snapshot(), server, session.server_host())
                        {
                            send(&mut stdin, &message)?;
                        }
                    }
                    match &update {
                        Update::Event(aqx_client::Event::Phase(Phase::Ready)) => {
                            offline = false;
                            if let Some(id) = awaiting_online.take() {
                                let _ = tx.send(Wake::Online(id, Ok(())));
                            }
                        }
                        Update::Event(aqx_client::Event::Phase(Phase::Closed { reason })) => {
                            if let Some(id) = awaiting_online.take() {
                                let _ = tx.send(Wake::Online(id, Err(stayed_offline(reason))));
                            }
                        }
                        _ => {}
                    }
                    match update {
                        Update::Event(aqx_client::Event::Phase(
                            Phase::Closed { reason } | Phase::Reconnecting { reason, .. },
                        )) if !offline
                            && !suppress_close
                            && relogging.is_none()
                            && stopping.is_none() =>
                        {
                            offline = true;
                            send(
                                &mut stdin,
                                &Message::Disconnected {
                                    disconnected: matches!(
                                        reason,
                                        aqx_client::Disconnect::Kicked { .. }
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
                            if let Err(aqx_client::ActionError::Closed(reason)) = &result
                                && !offline
                                && !suppress_close
                                && relogging.is_none()
                                && stopping.is_none()
                            {
                                offline = true;
                                send(
                                    &mut stdin,
                                    &Message::Disconnected {
                                        disconnected: matches!(
                                            reason,
                                            aqx_client::Disconnect::Kicked { .. }
                                        ),
                                    },
                                )?;
                            }
                            if let Some((request, _)) = actions.remove(&id) {
                                send(&mut stdin, &reply(request, result))?;
                            }
                        }
                        _ => {}
                    }
                }
                Wake::Update(..) => {}
                Wake::Ended(g) if g == generation => {
                    ended = true;
                    for (_, (request, pending)) in actions.drain() {
                        send(&mut stdin, &reply(request, pending.wait()))?;
                    }
                }
                Wake::Ended(_) => {}
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
                wake @ (Wake::Relogged(..) | Wake::Online(..)) => {
                    let (reply, result) = match wake {
                        Wake::Relogged(reply, result) => (reply, result.map(Relogged::take)),
                        Wake::Online(reply, result) => {
                            (reply, result.map(|()| (session.clone(), server.clone())))
                        }
                        _ => unreachable!(),
                    };
                    if !relogging.is_some_and(|(id, _)| id == reply) {
                        continue;
                    }
                    relogging = None;
                    match result {
                        Ok((next, name)) => {
                            for (_, (reply, _)) in actions.drain() {
                                send(
                                    &mut stdin,
                                    &Message::Error {
                                        reply,
                                        error: aqx_client::ActionError::Cancelled.into(),
                                    },
                                )?;
                            }
                            *session = next;
                            suppress_close = false;
                            ended = false;
                            offline = false;
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
fn stayed_offline(reason: &aqx_client::Disconnect) -> String {
    format!("the account stayed offline: {reason}")
}
fn reply(reply: RequestId, result: Result<(), aqx_client::ActionError>) -> Message {
    match result {
        Ok(()) => Message::Ok {
            reply,
            ok: Value::Null,
        },
        Err(error) => Message::Error {
            reply,
            error: error.into(),
        },
    }
}
fn send(writer: &mut impl Write, message: &Message) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}
enum Wake {
    Worker(io::Result<Worker>),
    Exited,
    Log(String),
    Cancel,
    Update(u64, Update),
    Ended(u64),
    Servers(RequestId, Result<Vec<aqx_client::Server>, String>),
    Relogged(RequestId, Result<Relogged, String>),
    Online(RequestId, Result<(), String>),
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
        .name("aqx-skua-updates".into())
        .spawn(move || {
            while current.load(Ordering::Acquire) == generation {
                match updates.recv_timeout(Duration::from_millis(100)) {
                    Ok(update) => {
                        if tx.send(Wake::Update(generation, update)).is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = tx.send(Wake::Ended(generation));
                        break;
                    }
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
    fn each_message_reaches_the_worker_as_one_write() {
        #[derive(Default)]
        struct Pipe {
            writes: Vec<Vec<u8>>,
        }
        impl Write for Pipe {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.writes.push(bytes.to_vec());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut pipe = Pipe::default();
        let event = json!({"type": "joined", "map": "battleon", "cell": "Enter", "pad": "Spawn"});
        send(
            &mut pipe,
            &Message::Event {
                event: event.clone(),
            },
        )
        .unwrap();
        send(&mut pipe, &Message::Event { event }).unwrap();
        assert_eq!(pipe.writes.len(), 2);
        for line in &pipe.writes {
            let text = std::str::from_utf8(line).unwrap();
            assert!(text.ends_with('\n'));
            assert!(text.contains(r#""map":"battleon""#));
        }
    }

    #[test]
    fn accounts_starting_together_build_a_stale_worker_once() {
        let root = std::env::temp_dir().join(format!("aqx-worker-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let skua = root.join("skua");
        std::fs::create_dir_all(skua.join("Host")).unwrap();
        std::fs::write(skua.join("Host/Host.csproj"), "").unwrap();
        let builds = root.join("builds");
        let dotnet = root.join("dotnet");
        std::fs::write(
            &dotnet,
            format!(
                "#!/bin/sh\necho >> '{}'\nsleep 0.3\nwhile [ \"$1\" != -o ]; do shift; done\nmkdir -p \"$2\" && touch \"$2/aqx-skua\"\n",
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
