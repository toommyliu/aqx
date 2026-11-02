use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use aqx_core::{ActError, Event, Halt, ItemRef, Outcome, ScriptError, Session, StopHandle, views};
use aqx_protocol::{ItemId, MonMapId, QuestId, ShopId};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const GRACE: Duration = Duration::from_secs(3);
const REAP_POLL: Duration = Duration::from_millis(50);

#[derive(Deserialize)]
#[serde(untagged)]
enum FromHost {
    Call {
        id: u64,
        call: String,
        #[serde(default)]
        args: Value,
    },
    Done {
        done: End,
    },
}

#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum End {
    Finished,
    Stopped { reason: Option<String> },
    Failed { error: String },
}

type Reply = Result<Value, Failure>;
type Handler = fn(&mut Session, &Value) -> Reply;

enum Op {
    Instant(Handler),
    Blocking(Handler),
}

enum Inbound {
    Call { id: u64, run: Handler, args: Value },
    Done(End),
    Closed(String),
}

struct Failure {
    code: &'static str,
    message: String,
    halt: Option<Halt>,
}

impl From<ActError> for Failure {
    fn from(error: ActError) -> Self {
        let message = error.to_string();
        match error {
            ActError::Timeout(_) => Failure { code: "timeout", message, halt: None },
            ActError::NotFound(_) => Failure { code: "not_found", message, halt: None },
            ActError::Rejected(_) => Failure { code: "rejected", message, halt: None },
            ActError::Halted(halt) => Failure { code: "cancelled", message, halt: Some(halt) },
        }
    }
}

fn invalid(message: String) -> Failure {
    Failure { code: "invalid", message, halt: None }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Item {
    Name(String),
    Id(ItemId),
}

impl From<Item> for ItemRef {
    fn from(item: Item) -> Self {
        match item {
            Item::Name(name) => ItemRef::Name(name),
            Item::Id(id) => ItemRef::Id(id),
        }
    }
}

#[derive(Clone)]
struct Host(Arc<Mutex<Box<dyn Write + Send>>>);

impl Host {
    fn send(&self, message: &Value) {
        if let Ok(mut out) = self.0.lock() {
            let _ = writeln!(out, "{message}").and_then(|()| out.flush());
        }
    }

    fn reply(&self, id: u64, reply: &Reply) {
        self.send(&match reply {
            Ok(value) => json!({ "id": id, "ok": value }),
            Err(failure) => json!({ "id": id, "error": { "code": failure.code, "message": failure.message } }),
        });
    }
}

pub fn run(session: &mut Session, mut host: Command) -> Result<Outcome, ScriptError> {
    let program = host.get_program().to_string_lossy().into_owned();
    let mut child = host
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ScriptError::Load(format!("couldn't start {program}: {e}")))?;
    let (Some(to_host), Some(from_host), Some(errors)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err(ScriptError::Load(format!("{program} started without pipes")));
    };
    forward_errors(errors, session.stop_handle());
    let result = serve(session, from_host, to_host);
    reap(session, &mut child);
    result
}

pub fn serve(session: &mut Session, from_host: impl Read + Send + 'static, to_host: impl Write + Send + 'static) -> Result<Outcome, ScriptError> {
    let host = Host(Arc::new(Mutex::new(Box::new(to_host))));
    let (inbound, calls) = mpsc::channel();
    read_host(from_host, host.clone(), session.stop_handle(), inbound);
    let events = host.clone();
    session.set_hook(Some(Box::new(move |s: &mut Session| {
        for event in s.take_script_events() {
            if let Some(name) = views::event_name(&event) {
                events.send(&json!({ "event": name, "data": views::event(&s.world, &event) }));
            }
        }
    })));

    let result = loop {
        let next = match session.pump_until(None, |_| calls.try_recv().ok()) {
            Ok(next) => next,
            Err(halt) => break Err(ScriptError::Halted(halt)),
        };
        match next {
            Inbound::Call { id, run, args } => match run(session, &args) {
                Err(Failure { halt: Some(halt), code, message }) => {
                    host.reply(id, &Err(Failure { code, message, halt: None }));
                    break Err(ScriptError::Halted(halt));
                }
                reply => host.reply(id, &reply),
            },
            Inbound::Done(End::Finished) => break Ok(Outcome::Finished),
            Inbound::Done(End::Stopped { reason }) => break Ok(Outcome::Stopped(reason)),
            Inbound::Done(End::Failed { error }) => break Err(ScriptError::Failed(error)),
            Inbound::Closed(why) => break Err(ScriptError::Failed(why)),
        }
    };
    if let Err(ScriptError::Halted(halt)) = &result {
        host.send(&json!({ "stop": format!("{halt:?}") }));
    }
    session.set_hook(None);
    result
}

fn read_host(from_host: impl Read + Send + 'static, host: Host, handle: StopHandle, inbound: Sender<Inbound>) {
    thread::spawn(move || {
        let deliver = |message: Inbound| {
            let _ = inbound.send(message);
            handle.post(|_| {});
        };
        for line in BufReader::new(from_host).lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<FromHost>(&line) {
                Ok(FromHost::Call { id, call, args }) => match op(&call) {
                    Some(Op::Instant(run)) => {
                        let host = host.clone();
                        handle.post(move |s| host.reply(id, &run(s, &args)));
                    }
                    Some(Op::Blocking(run)) => deliver(Inbound::Call { id, run, args }),
                    None => host.reply(id, &Err(invalid(format!("there is no call named {call}")))),
                },
                Ok(FromHost::Done { done }) => return deliver(Inbound::Done(done)),
                Err(e) => return deliver(Inbound::Closed(format!("the host sent an unreadable message ({e}): {line}"))),
            }
        }
        deliver(Inbound::Closed("the host exited without finishing".into()));
    });
}

fn forward_errors(errors: impl Read + Send + 'static, handle: StopHandle) {
    thread::spawn(move || {
        for line in BufReader::new(errors).lines().map_while(Result::ok) {
            handle.post(move |s| (s.sink())(&Event::ScriptLog { level: "warn".into(), text: line }));
        }
    });
}

fn reap(session: &mut Session, child: &mut Child) {
    let deadline = Instant::now() + GRACE;
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        if session.pump_until(Some(Instant::now() + REAP_POLL), |_| None::<()>).is_err() {
            thread::sleep(REAP_POLL);
        }
    }
}

fn args<T: DeserializeOwned>(args: &Value) -> Result<T, Failure> {
    T::deserialize(args).map_err(|e| invalid(format!("bad arguments {args}: {e}")))
}

fn done() -> Reply {
    Ok(Value::Null)
}

fn emit(session: &Session, event: Event) -> Reply {
    (session.sink())(&event);
    done()
}

fn op(name: &str) -> Option<Op> {
    use Op::{Blocking as B, Instant as I};
    Some(match name {
        "view" => I(view),
        "quest" => I(|s, a| {
            let (id,): (QuestId,) = args(a)?;
            Ok(s.world.quests.get(&id).map_or(Value::Null, |q| views::quest(&s.world, q)))
        }),
        "shop" => I(|s, a| {
            let (id,): (ShopId,) = args(a)?;
            Ok(s.world.shops.get(&id).map_or(Value::Null, |shop| views::shop(shop)))
        }),
        "server" => I(|s, _| Ok(s.server().map_or(Value::Null, views::server))),
        "target" => I(|s, a| {
            let (id,): (Option<MonMapId>,) = args(a)?;
            s.set_target(id);
            done()
        }),
        "can_cast" => I(|s, a| {
            let (slot,): (usize,) = args(a)?;
            Ok(json!(s.can_cast(slot)))
        }),
        "cast" => I(|s, a| {
            let (slot,): (usize,) = args(a)?;
            Ok(json!(s.cast_now(slot)))
        }),
        "listen" => I(|s, a| {
            let (cmd, on): (String, bool) = args(a)?;
            s.listen(&cmd, on);
            done()
        }),
        "status" => I(|s, a| {
            let (text,): (String,) = args(a)?;
            emit(s, Event::Status { text })
        }),
        "progress" => I(|s, a| {
            let (label, current, total): (String, u32, u32) = args(a)?;
            emit(s, Event::Progress { label, current, total })
        }),
        "log" => I(|s, a| {
            let (level, text): (String, String) = args(a)?;
            emit(s, Event::ScriptLog { level, text })
        }),
        "join" => B(|s, a| {
            let (map,): (String,) = args(a)?;
            s.join(&map)?;
            done()
        }),
        "jump" => B(|s, a| {
            let (cell, pad): (String, String) = args(a)?;
            s.jump(&cell, &pad)?;
            done()
        }),
        "walk" => B(|s, a| {
            let (x, y): (i32, i32) = args(a)?;
            s.walk(x, y)?;
            done()
        }),
        "goto" => B(|s, a| {
            let (player,): (String,) = args(a)?;
            s.goto(&player)?;
            done()
        }),
        "rest" => B(|s, _| {
            s.rest()?;
            done()
        }),
        "exit_combat" => B(|s, _| {
            s.exit_combat()?;
            done()
        }),
        "load_quests" => B(|s, a| {
            let (ids,): (Vec<QuestId>,) = args(a)?;
            s.load_quests(&ids)?;
            done()
        }),
        "accept" => B(|s, a| {
            let (id,): (QuestId,) = args(a)?;
            s.accept(id)?;
            done()
        }),
        "complete" => B(|s, a| {
            let (id, reward, times): (QuestId, Option<ItemId>, u32) = args(a)?;
            s.complete(id, reward, times)?;
            done()
        }),
        "pickup" => B(|s, a| {
            let (name,): (String,) = args(a)?;
            s.pickup(&name)?;
            done()
        }),
        "collect" => B(|s, a| {
            let (map_item, item, qty): (u32, Item, u32) = args(a)?;
            s.collect(map_item, &item.into(), qty)?;
            done()
        }),
        "load_bank" => B(|s, _| {
            s.load_bank()?;
            done()
        }),
        "withdraw" => B(|s, a| {
            let (item,): (Item,) = args(a)?;
            s.withdraw(&item.into())?;
            done()
        }),
        "deposit" => B(|s, a| {
            let (item,): (Item,) = args(a)?;
            s.deposit(&item.into())?;
            done()
        }),
        "load_shop" => B(|s, a| {
            let (id,): (ShopId,) = args(a)?;
            s.load_shop(id)?;
            done()
        }),
        "buy" => B(|s, a| {
            let (shop, item, qty): (ShopId, Item, u32) = args(a)?;
            s.buy(shop, &item.into(), qty)?;
            done()
        }),
        "sell" => B(|s, a| {
            let (item, qty): (Item, u32) = args(a)?;
            s.sell(&item.into(), qty)?;
            done()
        }),
        "equip" => B(|s, a| {
            let (item,): (Item,) = args(a)?;
            s.equip(&item.into())?;
            done()
        }),
        "use_item" => B(|s, a| {
            let (item,): (Item,) = args(a)?;
            s.use_item(&item.into())?;
            done()
        }),
        "send" => B(|s, a| {
            let (packet,): (String,) = args(a)?;
            s.send_raw(&packet)?;
            done()
        }),
        "relogin" => B(|s, a| {
            let (server,): (Option<String>,) = args(a)?;
            s.relogin(server.as_deref())?;
            done()
        }),
        _ => return None,
    })
}

fn view(session: &mut Session, a: &Value) -> Reply {
    let (name,): (String,) = args(a)?;
    let world = &session.world;
    Ok(match name.as_str() {
        "player" => views::player(world),
        "map" => views::map(world),
        "inventory" => views::items(&world.inventory),
        "temp" => views::items(&world.temp),
        "drops" => views::items(&world.drops),
        "bank" if world.bank_loaded => views::items(&world.bank),
        "bank" => Value::Null,
        "skills" => views::skills(world),
        "slots" => views::slots(world),
        _ => return Err(invalid(format!("there is no view named {name}"))),
    })
}
