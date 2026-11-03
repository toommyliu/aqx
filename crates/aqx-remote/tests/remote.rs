#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use aqx_core::{Event, Halt, Outcome, ScriptError, Sink};
use aqx_protocol::QuestId;
use serde_json::{Value, json};
use server::{FARM, Fake, Wire};

struct Host {
    to_aqx: Option<PipeWriter>,
    from_aqx: BufReader<PipeReader>,
    unread: Vec<Value>,
}

impl Host {
    fn send(&mut self, message: Value) {
        writeln!(self.to_aqx.as_ref().unwrap(), "{message}").unwrap();
    }

    fn call(&mut self, id: u64, call: &str, args: Value) -> Value {
        self.send(json!({ "id": id, "call": call, "args": args }));
        self.wait(|m| m["id"] == id)
    }

    fn wait(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        if let Some(i) = self.unread.iter().position(&wanted) {
            return self.unread.remove(i);
        }
        loop {
            let mut line = String::new();
            assert_ne!(self.from_aqx.read_line(&mut line).unwrap(), 0, "aqx closed the pipe; unread: {:?}", self.unread);
            let message: Value = serde_json::from_str(&line).unwrap();
            if wanted(&message) {
                return message;
            }
            self.unread.push(message);
        }
    }

    fn next(&mut self) -> Value {
        self.wait(|_| true)
    }

    fn finish(&mut self) {
        self.send(json!({ "done": { "outcome": "finished" } }));
    }
}

fn remote(fake: &mut Fake, script: impl FnOnce(&mut Host) + Send + 'static) -> Result<Outcome, ScriptError> {
    let (aqx_reads, host_writes) = std::io::pipe().unwrap();
    let (host_reads, aqx_writes) = std::io::pipe().unwrap();
    let host = thread::spawn(move || {
        let mut host = Host { to_aqx: Some(host_writes), from_aqx: BufReader::new(host_reads), unread: Vec::new() };
        script(&mut host);
    });
    let result = aqx_remote::serve(&mut fake.session, aqx_reads, aqx_writes);
    host.join().expect("the host's checks failed");
    result
}

fn kills_on_first_hit(msg: &str, wire: &mut Wire) {
    if msg.contains("%gar%") {
        wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
        wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
    }
}

#[test]
fn calls_reach_the_session_and_events_reach_the_host() {
    let mut fake = Fake::start(FARM, kills_on_first_hit);
    let result = remote(&mut fake, |host| {
        assert_eq!(host.call(1, "view", json!(["player"]))["ok"]["name"], "hero");
        assert_eq!(host.call(2, "jump", json!(["r2", "Left"])), json!({ "id": 2, "ok": null }));
        host.call(3, "target", json!([2]));
        let kill = host.wait(|m| m["event"] == "kill");
        assert_eq!((kill["data"]["mapId"].as_u64(), kill["data"]["name"].as_str()), (Some(2), Some("Treeant")));
        assert_eq!(host.call(4, "view", json!(["player"]))["ok"]["cell"], "r2");
        host.finish();
    });
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}");
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    assert!(sent.iter().any(|m| m == "%xt%zm%moveToCell%5%r2%Left%"), "{sent:#?}");
    assert!(sent.iter().any(|m| m.contains("%aa>m:2%")), "{sent:#?}");
}

#[test]
fn an_instant_call_is_answered_while_a_blocking_call_waits() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = remote(&mut fake, |host| {
        host.send(json!({ "id": 1, "call": "jump", "args": ["r1", "Left"] }));
        host.send(json!({ "id": 2, "call": "view", "args": ["map"] }));
        let mut replies = Vec::new();
        while replies.len() < 2 {
            let message = host.next();
            if message.get("id").is_some() {
                replies.push(message);
            }
        }
        assert_eq!(replies[0]["id"], 2, "the view waited behind the jump: {replies:?}");
        assert_eq!(replies[0]["ok"]["name"], "farm");
        assert_eq!(replies[1], json!({ "id": 1, "ok": null }));
        host.finish();
    });
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}");
}

#[test]
fn bad_and_failing_calls_come_back_as_errors() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = remote(&mut fake, |host| {
        let code = |reply: Value| reply["error"]["code"].as_str().unwrap_or_default().to_string();
        assert_eq!(code(host.call(1, "fly", json!([]))), "invalid");
        assert_eq!(code(host.call(2, "jump", json!([1]))), "invalid");
        assert_eq!(code(host.call(3, "equip", json!(["Nothing"]))), "not_found");
        assert_eq!(host.call(4, "view", json!(["bank"]))["ok"], Value::Null, "the bank isn't loaded yet");
        host.send(json!({ "done": { "outcome": "stopped", "reason": "enough" } }));
    });
    assert!(matches!(&result, Ok(Outcome::Stopped(Some(why))) if why == "enough"), "{result:?}");
}

#[test]
fn stopping_the_account_cancels_the_call_and_tells_the_host() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let handle = fake.session.stop_handle();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        handle.stop();
    });
    let result = remote(&mut fake, |host| {
        let reply = host.call(1, "join", json!(["nowhere"]));
        assert_eq!(reply["error"]["code"], "cancelled", "{reply}");
        assert!(host.next().get("stop").is_some());
    });
    assert!(matches!(result, Err(ScriptError::Halted(Halt::Stopped))), "{result:?}");
}

#[test]
fn a_host_that_exits_without_finishing_fails_the_run() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = remote(&mut fake, |host| {
        host.call(1, "view", json!(["player"]));
        host.to_aqx = None;
    });
    assert!(matches!(&result, Err(ScriptError::Failed(why)) if why.contains("without finishing")), "{result:?}");
}

#[test]
fn run_talks_to_a_child_process_and_logs_what_it_prints_to_stderr() {
    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink: Sink = {
        let logged = logged.clone();
        Rc::new(move |event: &Event| {
            if let Event::ScriptLog { level, text } = event {
                logged.lock().unwrap().push(format!("{level}: {text}"));
            }
        })
    };
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let mut host = Command::new("sh");
    host.args([
        "-c",
        r#"echo "compiling" >&2
           echo '{"id":1,"call":"view","args":["player"]}'
           read reply
           case "$reply" in *'"name":"hero"'*) ;; *) exit 1 ;; esac
           echo '{"done":{"outcome":"stopped","reason":"saw hero"}}'"#,
    ]);
    let result = aqx_remote::run(&mut fake.session, host);
    assert!(matches!(&result, Ok(Outcome::Stopped(Some(why))) if why == "saw hero"), "{result:?}");
    assert!(logged.lock().unwrap().iter().any(|line| line == "warn: compiling"), "{:?}", logged.lock().unwrap());
}

#[test]
fn abandoning_a_quest_drops_it_from_the_active_list_without_a_packet() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.world.active.push(QuestId(11));
    let result = remote(&mut fake, |host| {
        assert_eq!(host.call(1, "abandon", json!([11])), json!({ "id": 1, "ok": null }));
        host.finish();
    });
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}");
    assert!(fake.session.world.active.is_empty());
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    assert!(!sent.iter().any(|m| m.contains("%11%")), "{sent:#?}");
}
