use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mira_client::testing::{Harness, Peer, json_message, move_to_area};
use mira_client::{ActionError, Aim, MapItemId, MonMapId, Options, Update};
use mira_client::{Handle, Server};
use mira_protocol::packet;
use mira_protocol::{AreaId, EntityId, ItemId, QuestId, ShopId};
use mira_script::{Account, Cancel, Level, LoadError, Outcome, Output, Script, Start};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

mod api_types;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mira-script-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn file(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, source).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn serve(script: impl FnOnce(&mut Peer) + Send + 'static) -> Harness {
    let harness = Harness::start(Options::default(), move |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready();
        script(&mut peer);
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    harness
}

fn farm(script: impl FnOnce(&mut Peer) + Send + 'static) -> Harness {
    let harness = Harness::start(Options::default(), move |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        script(&mut peer);
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness
        .session
        .snapshot()
        .room
        .as_ref()
        .map(|r| r.name.as_str())
        != Some("farm-1")
    {
        assert!(Instant::now() < deadline, "the farm never loaded");
        thread::sleep(Duration::from_millis(5));
    }
    harness
}

fn run_on(harness: Harness, source: &str) -> (Result<Outcome, LoadError>, Vec<Output>) {
    let dir = Dir::new();
    let path = dir.file("main.ts", source);
    let ran = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    ran
}

fn finish(harness: Harness) {
    harness.session.logout();
    harness.finish();
}

struct NoAccount;

impl Account for NoAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        Err("no server list in this test".into())
    }

    fn login(&self, _: &str) -> Result<Handle, String> {
        Err("no logins in this test".into())
    }
}

fn run_in(
    harness: &Harness,
    path: &Path,
    cancel: &Cancel,
    output: impl FnMut(&Output) + 'static,
) -> (Result<Outcome, LoadError>, Vec<Output>) {
    run_with(harness, path, cancel, &[], Arc::new(NoAccount), output)
}

fn run_with(
    harness: &Harness,
    path: &Path,
    cancel: &Cancel,
    args: &[(&str, &str)],
    account: Arc<dyn Account>,
    mut output: impl FnMut(&Output) + 'static,
) -> (Result<Outcome, LoadError>, Vec<Output>) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let script = Script::load(
        path,
        move |out| {
            output(&out);
            sink.borrow_mut().push(out);
        },
        cancel,
    );
    let outcome = script.and_then(|script| {
        let given: Vec<(String, String)> = args
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let args = script
            .parse_args(&given)
            .map_err(|why| panic!("bad test args: {why}"))?;
        let start = Start {
            session: harness.session.clone(),
            server: "Test".into(),
            args,
            account,
        };
        Ok(script.run(start).outcome)
    });
    (outcome, seen.take())
}

fn load_error(source: &str) -> String {
    let dir = Dir::new();
    let path = dir.file("main.ts", source);
    match Script::load(&path, |_| {}, &Cancel::default()) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected {source:?} to fail to load"),
    }
}

fn run_path(path: &Path, cancel: &Cancel) -> (Result<Outcome, LoadError>, Vec<Output>) {
    let harness = serve(|_| {});
    let ran = run_in(&harness, path, cancel, |_| {});
    finish(harness);
    ran
}

fn run_script(source: &str) -> (Result<Outcome, LoadError>, Vec<Output>) {
    let dir = Dir::new();
    run_path(&dir.file("main.ts", source), &Cancel::default())
}

fn logs(outputs: &[Output]) -> Vec<&str> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::Log { text, .. } => Some(text.as_str()),
            Output::Status(_) => None,
        })
        .collect()
}

fn failure(outcome: Result<Outcome, LoadError>) -> String {
    match outcome {
        Ok(Outcome::Failed(why)) => why,
        other => panic!("expected the run to fail, got {other:?}"),
    }
}

#[test]
fn the_mira_version_is_available_before_login_and_during_a_run() {
    let (outcome, outputs) = run_script(
        r#"
        import { MIRA_VERSION, script } from "mira";
        script.log(MIRA_VERSION);
        export default function* () {
          script.log(MIRA_VERSION);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_VERSION")]
    );
}

#[test]
fn version_compare_uses_semantic_version_precedence() {
    let (outcome, _) = run_script(
        r#"
        import { MIRA_VERSION, versionCompare } from "mira";
        const cases: [string, string, -1 | 0 | 1][] = [
          [MIRA_VERSION, MIRA_VERSION, 0],
          ["1.2.3", "1.2.3", 0],
          ["1.2.3", "2.0.0", -1],
          ["1.10.0", "1.2.0", 1],
          ["1.2.10", "1.2.9", 1],
          ["1.0.0-alpha", "1.0.0-alpha.1", -1],
          ["1.0.0-alpha.1", "1.0.0-alpha.beta", -1],
          ["1.0.0-alpha.beta", "1.0.0-beta", -1],
          ["1.0.0-beta.2", "1.0.0-beta.11", -1],
          ["1.0.0-beta.11", "1.0.0-rc.1", -1],
          ["1.0.0-rc.1", "1.0.0", -1],
          ["1.0.0+build.1", "1.0.0+build.2", 0],
          ["1.0.0-alpha+build.1", "1.0.0-alpha+build.2", 0],
          ["1.0.0-alpha", "1.0.0-Beta", 1],
          ["1.0.9007199254740992", "1.0.9007199254740993", -1],
          ["1.0.0-9007199254740992", "1.0.0-9007199254740993", -1],
        ];
        for (const [left, right, expected] of cases) {
          if (versionCompare(left, right) !== expected || versionCompare(right, left) !== -expected) {
            throw new Error(`wrong precedence for ${left} and ${right}`);
          }
        }
        export default function* () {}
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
}

#[test]
fn version_compare_rejects_invalid_versions_and_non_strings() {
    let (outcome, _) = run_script(
        r#"
        import { versionCompare } from "mira";
        for (const value of ["", "1", "1.2", "v1.2.3", "01.2.3", "1.2.3.4", "1.2.3-01", "1.2.3+", " 1.2.3", "1.2.3\n", 123, null, undefined, {}]) {
          for (const [left, right] of [[value, "1.2.3"], ["1.2.3", value]]) {
            let error: unknown;
            try {
              versionCompare(left, right);
            } catch (e) {
              error = e;
            }
            if (typeof value === "string" ? !(error instanceof RangeError) : !(error instanceof TypeError)) {
              throw new Error(`wrong error for ${JSON.stringify([left, right])}`);
            }
          }
        }
        export default function* () {}
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
}

#[test]
fn a_script_logs_with_levels_and_sets_its_status() {
    let (outcome, outputs) = run_script(
        r#"
        import { script } from "mira";
        interface Point { x: number }
        const origin: Point = { x: 0 };
        export default function* (): Generator {
          script.log("at", 1, origin, [true, null]);
          script.warn("careful");
          script.error(new RangeError("too far"));
          script.status("farming");
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        outputs,
        [
            Output::Log {
                level: Level::Log,
                text: r#"at 1 {"x":0} [true,null]"#.into()
            },
            Output::Log {
                level: Level::Warn,
                text: "careful".into()
            },
            Output::Log {
                level: Level::Error,
                text: "RangeError: too far".into()
            },
            Output::Status("farming".into()),
        ]
    );
}

#[test]
fn wait_ms_resumes_the_script_after_the_delay() {
    let started = Instant::now();
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        export default function* () {
          yield* wait.ms(50);
          script.log("waited");
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["waited"]);
    assert!(started.elapsed() >= Duration::from_millis(50));
}

#[test]
fn a_generator_of_the_scripts_own_is_an_action() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        function* double(n: number) {
          yield* wait.ms(1);
          return n * 2;
        }
        const callable = Object.assign(() => {}, { [Symbol.iterator]: () => double(4) });
        export default function* () {
          script.log(yield* double(21));
          script.log(JSON.stringify(yield* wait.first({ callable })));
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["42", r#"{"winner":"callable","value":8}"#]);
}

#[test]
fn wait_first_returns_the_winner_and_stops_the_rest() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        function* slow() {
          try {
            yield* wait.ms(500);
            script.log("slow finished");
          } finally {
            script.log("slow stopped");
          }
        }
        function* fast() {
          yield* wait.ms(10);
          return "fast value";
        }
        export default function* () {
          const won = yield* wait.first({ slow: slow(), fast: fast() });
          script.log(won.winner, won.value);
          yield* wait.ms(600);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["slow stopped", "fast fast value"]);
}

#[test]
fn wait_first_starts_nothing_more_once_an_action_finishes_at_once() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        function* now() {
          return 1;
        }
        function* later() {
          script.log("later started");
          yield* wait.ms(1);
        }
        export default function* () {
          const won = yield* wait.first({ now: now(), later: later() });
          script.log(won.winner);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["now"]);
}

#[test]
fn an_error_in_a_wait_first_action_reaches_the_caller() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        function* fails() {
          yield* wait.ms(5);
          throw new Error("broke");
        }
        function* idle() {
          try {
            yield* wait.ms(500);
          } finally {
            script.log("idle stopped");
          }
        }
        export default function* () {
          try {
            yield* wait.first({ fails: fails(), idle: idle() });
          } catch (e) {
            script.log("caught", (e as Error).message);
          }
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["idle stopped", "caught broke"]);
}

#[test]
fn a_number_that_cannot_apply_throws_when_the_action_is_made() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        export default function* () {
          for (const ms of [-1, NaN, Infinity]) {
            try {
              wait.ms(ms);
            } catch (e) {
              script.log((e as Error).name);
            }
          }
          try {
            wait.ms("5" as any);
          } catch (e) {
            script.log((e as Error).name);
          }
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        ["RangeError", "RangeError", "RangeError", "TypeError"]
    );
}

#[test]
fn an_action_without_the_star_throws_and_one_never_run_warns() {
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"import { script, wait } from "mira";
export default function* () {
  wait.ms(1);
  yield* wait.first({ a: wait.ms(1), b: wait.ms(5) });
  try {
    yield wait.ms(1);
  } catch (e) {
    script.log((e as Error).name, (e as Error).message);
  }
}
"#,
    );
    let (outcome, outputs) = run_path(&path, &Cancel::default());
    assert_eq!(outcome, Ok(Outcome::Finished));
    let main = path.display();
    let shown: Vec<(Level, String)> = outputs
        .into_iter()
        .filter_map(|output| match output {
            Output::Log { level, text } => Some((level, text)),
            Output::Status(_) => None,
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                Level::Warn,
                format!("an action at {main}:3:8 never ran; run it with yield*")
            ),
            (
                Level::Warn,
                format!("an action at {main}:6:16 never ran; run it with yield*")
            ),
            (
                Level::Log,
                "TypeError run an action with yield*, not yield".into()
            ),
        ]
    );
}

#[test]
fn stop_ends_the_run_with_its_reason_and_runs_finally_blocks() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        export default function* () {
          try {
            script.stop("out of potions");
          } finally {
            script.log("cleaned up");
          }
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Stopped(Some("out of potions".into()))));
    assert_eq!(logs(&outputs), ["cleaned up"]);
}

#[test]
fn catching_stop_does_not_keep_the_run_going() {
    let (outcome, outputs) = run_script(
        r#"
        import { script, wait } from "mira";
        export default function* () {
          try {
            try {
              script.stop();
            } catch {}
            yield* wait.ms(1);
            script.log("kept going");
          } finally {
            script.log("cleaned up");
          }
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Stopped(None)));
    assert_eq!(logs(&outputs), ["cleaned up"]);
}

#[test]
fn an_uncaught_error_fails_the_run_with_the_scripts_stack() {
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { wait } from "mira";
        export default function* () {
          yield* wait.ms(1);
          throw new Error("boom");
        }
        "#,
    );
    let (outcome, _) = run_path(&path, &Cancel::default());
    let why = failure(outcome);
    assert!(why.starts_with("Error: boom\n"), "{why}");
    assert!(why.contains("main.ts"), "{why}");
    assert!(!why.contains("(mira"), "{why}");
}

#[test]
fn scripts_import_relative_files() {
    let dir = Dir::new();
    dir.file("util.ts", "export const greeting: string = \"hello\";");
    let path = dir.file(
        "main.ts",
        r#"
        import { script } from "mira";
        import { greeting } from "./util.ts";
        export default function* () {
          script.log(greeting);
        }
        "#,
    );
    let (outcome, outputs) = run_path(&path, &Cancel::default());
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["hello"]);
}

#[test]
fn a_script_cannot_import_other_modules() {
    let why = load_error(
        r#"
        import { readFileSync } from "fs";
        export default function* () {}
        "#,
    );
    assert!(why.contains("only \"mira\" and relative paths"), "{why}");
}

#[test]
fn the_default_export_must_be_a_generator_function() {
    for source in [
        "export default function () {}",
        "export default async function () {}",
        "export const x = 1;",
    ] {
        assert_eq!(
            load_error(source),
            "the script's default export must be a generator function",
            "{source}"
        );
    }
}

#[test]
fn broken_source_fails_to_load_before_any_session() {
    for source in [
        "enum Color { Red }\nexport default function* () {}",
        "export default function* () { let x: = 1 }",
        "import { nope } from \"mira\";\nexport default function* () {}",
    ] {
        assert!(!load_error(source).is_empty(), "{source}");
    }
}

#[test]
fn module_code_cannot_read_game_state_before_the_run() {
    let why = load_error(
        r#"
        import { player } from "mira";
        const hp = player.hp;
        export default function* () {}
        "#,
    );
    assert!(
        why.starts_with("MiraError: the character has not loaded yet"),
        "{why}"
    );
    let why = load_error(
        r#"
        import { script } from "mira";
        const { name } = script.args<{ name: string }>();
        export default function* () {}
        "#,
    );
    assert!(
        why.starts_with("MiraError: script.args can't run before the script starts"),
        "{why}"
    );
}

#[test]
fn cancelling_a_waiting_script_runs_its_finally_blocks() {
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { script, wait } from "mira";
        export default function* () {
          try {
            yield* wait.ms(60_000);
          } finally {
            script.log("cleaned up");
          }
        }
        "#,
    );
    let cancel = Cancel::default();
    let canceller = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        canceller.cancel();
    });
    let (outcome, outputs) = run_path(&path, &cancel);
    assert_eq!(outcome, Ok(Outcome::Cancelled));
    assert_eq!(logs(&outputs), ["cleaned up"]);
}

#[test]
fn cancelling_interrupts_a_script_stuck_in_a_loop() {
    let dir = Dir::new();
    let path = dir.file("main.ts", "export default function* () { while (true) {} }");
    let cancel = Cancel::default();
    let canceller = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        canceller.cancel();
    });
    let (outcome, _) = run_path(&path, &cancel);
    assert_eq!(outcome, Ok(Outcome::Cancelled));
}

#[test]
fn actions_change_the_state_the_script_reads() {
    let harness = serve(|peer| {
        peer.expect(&packet::transfer("player1", "yulgar-2"));
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { map, player, script } from "mira";
        export default function* () {
          script.log(map.name);
          script.log(yield* map.join("yulgar-2"));
          script.log(map.name, player.name);
          yield* map.jump("r2");
          script.log(map.cell, map.pad);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        ["battleon-1", "joined", "yulgar-2 player1", "r2 Left"]
    );
}

#[test]
fn a_refused_join_answers_refused() {
    let harness = serve(|peer| {
        peer.expect(&packet::transfer("player1", "doomvault-1"));
        peer.send("%xt%warning%-1%Cannot join that map.%");
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { map, script } from "mira";
        export default function* () {
          script.log(yield* map.join("doomvault-1"), map.name);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["refused battleon-1"]);
}

#[test]
fn state_changes_only_at_a_yield() {
    let (read, damaged) = mpsc::channel::<()>();
    let harness = serve(move |peer| {
        damaged.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player1": {"intHP": 700}}}),
        ));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { player, script, wait } from "mira";
        export default function* () {
          const before = player.hp;
          script.log("read");
          const until = Date.now() + 300;
          while (Date.now() < until) {}
          script.log(before, player.hp);
          yield* wait.ms(0);
          script.log(player.hp);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), move |out| {
        if matches!(out, Output::Log { text, .. } if text == "read") {
            read.send(()).unwrap();
        }
    });
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["read", "950 950", "700"]);
}

#[test]
fn a_closed_session_ends_the_run_and_runs_finally_blocks() {
    let harness = serve(|_| {});
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { script, wait } from "mira";
        export default function* () {
          try {
            yield* wait.ms(60_000);
          } finally {
            script.log("cleaned up");
          }
        }
        "#,
    );
    let session = harness.session.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        session.logout();
    });
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    harness.finish();
    assert_eq!(outcome, Ok(Outcome::Disconnected("logged out".into())));
    assert_eq!(logs(&outputs), ["cleaned up"]);
}

#[test]
fn losing_a_wait_first_cancels_the_action() {
    let harness = serve(|peer| {
        peer.expect(&packet::transfer("player1", "yulgar-1"));
    });
    let updates = harness.session.updates();
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { map, script, wait } from "mira";
        export default function* () {
          const won = yield* wait.first({ join: map.join("yulgar-1"), quick: wait.ms(20) });
          script.log(won.winner);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["quick"]);
    let cancelled = updates.try_iter().any(|update| {
        matches!(
            update,
            Update::Done {
                result: Err(ActionError::Cancelled),
                ..
            }
        )
    });
    assert!(cancelled);
}

#[test]
fn views_show_the_player_and_the_map() {
    let harness = serve(|_| {});
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { map, player, script } from "mira";
        export default function* () {
          script.log(player.level, player.gold, player.class, player.stats.haste, player.auras.length);
          script.log(player.factions.map((f) => `${f.name} ${f.rep}`).join());
          script.log(map.areaId, map.players.map((p) => p.name).join(), map.monsters.length);
          script.log(player.stats === player.stats);
          script.log(map.players.map((p) => p.level));
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "50 1200  0.05 0",
            "Good 1000",
            "42 player2,player3 0",
            "true",
            "[null,10]",
        ]
    );
}

#[test]
fn countdowns_run_down_between_yields() {
    let (listening, ready) = mpsc::channel();
    let harness = serve(move |peer| {
        ready.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "xpboost", "op": "+", "iSecsLeft": 3600, "bShowShop": false}),
        ));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { player, script, wait } from "mira";
        export default function* () {
          script.log("listening");
          yield* wait.until(() => player.boosts.xp > 0);
          const before = player.boosts.xp;
          yield* wait.ms(50);
          script.log(player.boosts.xp < before);
        }
        "#,
    );
    let (outcome, outputs) = run_in(
        &harness,
        &path,
        &Cancel::default(),
        signal_on("listening", listening),
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["listening", "true"]);
}

#[test]
fn combat_shows_the_skills_and_the_target() {
    let (outcome, outputs) = run_on(
        farm(|_| {}),
        r#"
        import { combat, map, script } from "mira";
        export default function* () {
          for (const skill of combat.skills) {
            script.log(skill.slot, skill.name, skill.target, skill.maxTargets, skill.status.kind);
          }
          script.log(combat.target, map.monsters.map((m) => `${m.monMapId} ${m.name} ${m.cell} ${m.hp}`).join());
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "0 Auto Attack hostile 1 ready",
            "1 Strike hostile 2 ready",
            "2 Shield self 1 ready",
            "3 Meteor hostile 1 no_mana",
            "4 Ultimate hostile 1 locked",
            "null 1 Mosquito Enter 500,2 Mosquito Enter 200,3 Frogzard r2 800",
        ]
    );
}

#[test]
fn kill_fights_the_weakest_live_match_in_the_cell() {
    let (aimed, aims) = mpsc::channel();
    let harness = farm(move |peer| {
        aimed.send(peer.recv()).unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "m": {"2": {"intHP": 0, "intState": 0}}}),
        ));
    });
    let (outcome, outputs) = run_on(
        harness,
        r#"
        import { combat, script } from "mira";
        export default function* () {
          script.log(yield* combat.kill("Dragon"));
          script.log(yield* combat.kill("Frogzard"));
          script.log(yield* combat.kill("mosquito"));
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["none", "none", "killed"]);
    let weakest_first = [Aim::Monster(MonMapId(2)), Aim::Monster(MonMapId(1))];
    assert_eq!(aims.recv().unwrap(), packet::gar(0, "a1", &weakest_first));
}

#[test]
fn kill_answers_died_when_the_player_dies() {
    let harness = farm(|peer| {
        peer.recv();
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player1": {"intHP": 0, "intState": 0}}}),
        ));
    });
    let (outcome, outputs) = run_on(
        harness,
        r#"
        import { combat, player, script } from "mira";
        export default function* () {
          script.log(yield* combat.kill(1), player.state);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["died dead"]);
}

#[test]
fn combat_refuses_slots_and_targets_it_cannot_use() {
    let (outcome, outputs) = run_on(
        farm(|_| {}),
        r#"
        import { combat, script } from "mira";
        export default function* () {
          for (const make of [
            () => combat.useSkill(0 as any),
            () => combat.useSkill(6 as any),
            () => combat.useSkill("1" as any),
            () => combat.useSkill(1, { monster: 1, player: 7 } as any),
            () => combat.kill(-1),
            () => combat.kill("Mosquito", { skills: [0 as any] }),
            () => combat.kill("Mosquito", { skills: [] }),
          ]) {
            try {
              make();
            } catch (e) {
              script.log((e as Error).name);
            }
          }
          try {
            yield* combat.attack("Dragon");
          } catch (e) {
            script.log((e as Error).name, (e as Error).message);
          }
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "RangeError",
            "RangeError",
            "TypeError",
            "TypeError",
            "RangeError",
            "RangeError",
            "RangeError",
            "MiraError no live Dragon in this cell",
        ]
    );
}

#[test]
fn map_moves_follow_players_and_collect_items() {
    let (sent, frames) = mpsc::channel();
    let harness = Harness::start(Options::default(), move |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready();
        peer.expect(&packet::goto("nobody"));
        peer.send("%xt%warning%-1%Player 'nobody' could not be found.%");
        loop {
            let frame = peer.recv();
            let done = frame == packet::logout();
            sent.send(frame).unwrap();
            if done {
                break;
            }
        }
    });
    harness.session.ready().wait().unwrap();
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { map, script } from "mira";
        export default function* () {
          script.log(yield* map.goto("nobody"));
          script.log(yield* map.goto("player3"), map.cell);
          yield* map.walk(100, 200);
          script.log(map.x, map.y);
          yield* map.collect(5);
          map.setSpawnPoint("r2");
          for (const bad of [() => map.walk(1.5, 0), () => map.collect(-1), () => map.goto(3 as any)]) {
            try {
              bad();
            } catch (e) {
              script.log((e as Error).name);
            }
          }
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "not_found",
            "arrived r2",
            "100 200",
            "RangeError",
            "RangeError",
            "TypeError"
        ]
    );
    let frames: Vec<String> = frames.iter().collect();
    assert_eq!(
        frames,
        [
            packet::move_to_cell(AreaId(42), "r2", "Left"),
            packet::retrieve_user_datas(AreaId(42), &[EntityId(9)]),
            packet::walk(AreaId(42), 100, 200, 8),
            packet::get_map_item(AreaId(42), MapItemId(5)),
            packet::logout(),
        ]
    );
}

#[test]
fn inventory_and_drops_follow_the_bag() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_with_items(json!([
            {"ItemID": 100, "CharItemID": 5000, "sName": "Default Sword", "iQty": 1, "iStk": 1,
             "sType": "Sword", "sES": "Weapon", "bEquip": 1},
            {"ItemID": 102, "CharItemID": 5002, "sName": "Battle Axe", "iQty": 1, "iStk": 1,
             "sType": "Axe", "sES": "Weapon", "bEquip": 0},
            {"ItemID": 101, "CharItemID": 5001, "sName": "Health Potion", "iQty": 5,
             "iStk": 10, "sType": "Item", "sES": "None", "bEquip": 0},
        ]));
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {
            "300": {"ItemID": 300, "sName": "Mace", "iQty": 1, "iStk": 1,
                    "sType": "Mace", "sES": "Weapon"},
            "101": {"ItemID": 101, "sName": "Health Potion", "iQty": 2, "iStk": 10,
                    "sType": "Item", "sES": "None"},
        }})));
        peer.expect(&packet::get_drop(AreaId(42), ItemId(101)));
        peer.send(&json_message(json!({
            "cmd": "getDrop", "bSuccess": 1, "ItemID": 101, "CharItemID": 5001,
            "iQty": 2, "iQtyNow": 7, "bBank": 0,
        })));
        peer.expect("apply-enhancement");
        peer.send(&json_message(json!({
            "cmd": "enhanceItemShop", "ItemIDs": [100], "EnhID": 40514,
            "EnhPID": 9, "EnhLvl": 70, "EnhRty": 6, "ProcID": 3, "iCost": 400,
            "bSuccess": 1,
        })));
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { drops, inventory, packets, player, script, wait } from "mira";
        export default function* () {
          script.log(inventory.items.map((i) => `${i.name} ${i.qty} ${i.pool} ${i.equipped}`).join());
          script.log(inventory.get("health potion")?.id, inventory.count(101), inventory.count("Nothing"), inventory.bagSlots);
          while (drops.pending.length < 2) yield* wait.ms(5);
          for (const offer of drops.pending) script.log(offer.name, inventory.canFit(offer));
          script.log(yield* drops.pickup("Nope"));
          script.log(yield* drops.pickup(300));
          script.log(yield* drops.pickup("health potion"));
          script.log(inventory.count("Health Potion"), drops.pending.map((d) => d.name).join());
          yield* packets.send("apply-enhancement");
          yield* packets.once({ kind: "json", cmd: "enhanceItemShop" });
          script.log(inventory.get(100)?.enhancement, inventory.get(102)?.enhancement, player.gold, player.coins);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "Default Sword 1 bag true,Battle Axe 1 bag false,Health Potion 5 misc false",
            "101 5 0 2",
            "Health Potion true",
            "Mace false",
            "gone",
            "no_room",
            "picked",
            "7 Mace",
            "Lucky +5, Awe Blast null 800 30",
        ]
    );
}

#[test]
fn quests_load_accept_and_turn_in() {
    let harness = serve(|peer| {
        peer.expect(&packet::get_quests(AreaId(42), &[QuestId(30)]));
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {"30": {
            "QuestID": 30, "sName": "Potion Run",
            "turnin": [{"ItemID": 101, "iQty": 2}],
            "oItems": {"101": {"ItemID": 101, "sName": "Health Potion"}},
            "reward": [
                {"ItemID": 600, "iQty": 1, "iRate": 100, "iType": 2},
                {"ItemID": 601, "iQty": 1, "iRate": 100, "iType": 2},
            ],
        }}})));
        peer.expect(&packet::accept_quest(AreaId(42), QuestId(30)));
        peer.send(&json_message(
            json!({"cmd": "acceptQuest", "QuestID": 30, "bSuccess": 1, "msg": "success"}),
        ));
        peer.expect(&packet::try_quest_complete(
            AreaId(42),
            QuestId(30),
            Some(ItemId(600)),
            1,
        ));
        peer.send(&json_message(json!({"cmd": "turnIn", "sItems": "101:2"})));
        peer.send(&json_message(
            json!({"cmd": "ccqr", "QuestID": 30, "bSuccess": 1, "sName": "Potion Run"}),
        ));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { inventory, quests, script } from "mira";
        export default function* () {
          yield* quests.load(30);
          const quest = quests.get(30)!;
          script.log(quest.name, quest.status, quest.accepted, quest.turnIns);
          script.log(JSON.stringify(quest.turnIn), quest.rewards.map((r) => `${r.id} ${r.kind}`).join());
          yield* quests.accept(30);
          script.log(quests.get(30)!.accepted, quests.get(30)!.turnIns);
          yield* quests.complete(30, { reward: 600 });
          script.log(inventory.count(101), quests.get(31));
          try {
            quests.complete(30, { times: 1.5 });
          } catch (e) {
            script.log((e as Error).name);
          }
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "Potion Run available false 0",
            r#"[{"id":101,"name":"Health Potion","qty":2}] 600 chosen,601 chosen"#,
            "true 2",
            "3 undefined",
            "RangeError",
        ]
    );
}

#[test]
fn shop_loads_and_names_the_listing_to_buy() {
    let harness = serve(|peer| {
        peer.expect(&packet::load_shop(AreaId(42), ShopId(7)));
        peer.send(&json_message(json!({"cmd": "loadShop", "shopinfo": {
            "ShopID": 7, "sName": "Mixed", "Location": "battleon", "items": [
                {"ItemID": 101, "ShopItemID": 70, "sName": "Health Potion", "iCost": 100,
                 "bCoins": 0, "iStk": 10, "iQty": 1, "sType": "Item"},
                {"ItemID": 101, "ShopItemID": 75, "sName": "Health Potion", "iCost": 90,
                 "bCoins": 0, "iStk": 10, "iQty": 5, "sType": "Item"},
                {"ItemID": 905, "ShopItemID": 74, "sName": "Token Buy", "iCost": 0, "iStk": 10,
                 "iQty": 1, "TokID": 101, "TokQty": 2},
            ],
        }})));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { shop, script } from "mira";
        export default function* () {
          script.log(shop.current);
          const loaded = yield* shop.load(7);
          script.log(loaded.name, loaded.items.map((i) => `${i.listingId} ${i.qty} ${i.cost}`).join());
          script.log(JSON.stringify(loaded.items[2].token), shop.current?.id);
          for (const item of ["health potion", "Sword"]) {
            try {
              yield* shop.buy(item);
            } catch (e) {
              script.log((e as Error).name, (e as Error).message);
            }
          }
          try {
            shop.sell(101, 0);
          } catch (e) {
            script.log((e as Error).name);
          }
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), |_| {});
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "null",
            "Mixed 70 1 100,75 5 90,74 1 0",
            r#"{"id":101,"qty":2} 7"#,
            "MiraError more than one listing sells health potion; buy it by { listingId }",
            "MiraError Mixed doesn't sell Sword",
            "RangeError",
        ]
    );
}

fn signal_on(text: &'static str, signal: mpsc::Sender<()>) -> impl FnMut(&Output) + 'static {
    move |out| {
        if matches!(out, Output::Log { text: logged, .. } if logged == text) {
            let _ = signal.send(());
        }
    }
}

fn chat(text: &str) -> String {
    format!("%xt%chatm%-1%zone~{text}%player2%")
}

#[test]
fn events_reach_handlers_and_once_waiters() {
    let (listening, ready) = mpsc::channel();
    let harness = serve(move |peer| {
        ready.recv().unwrap();
        peer.send(&chat("one"));
        peer.send(&chat("two"));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { events, script } from "mira";
        export default function* () {
          events.on("chat", (e) => script.log("on", e.text));
          script.log("listening");
          const e = yield* events.once("chat", (c) => c.text === "two");
          script.log("once", e.sender, e.channel);
        }
        "#,
    );
    let (outcome, outputs) = run_in(
        &harness,
        &path,
        &Cancel::default(),
        signal_on("listening", listening),
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        ["listening", "on one", "on two", "once player2 zone"]
    );
}

#[test]
fn handler_actions_run_alongside_and_stop_when_removed() {
    let (listening, ready) = mpsc::channel();
    let harness = serve(move |peer| {
        ready.recv().unwrap();
        peer.send(&chat("one"));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { events, script, wait } from "mira";
        function* slow(e: { text: string }) {
          try {
            script.log("handling", e.text);
            yield* wait.ms(60_000);
          } finally {
            script.log("stopped", e.text);
          }
        }
        export default function* () {
          events.on("chat", slow);
          script.log("listening");
          yield* events.once("chat");
          yield* wait.ms(20);
          events.off("chat", slow);
          script.log("done");
        }
        "#,
    );
    let (outcome, outputs) = run_in(
        &harness,
        &path,
        &Cancel::default(),
        signal_on("listening", listening),
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        ["listening", "handling one", "done", "stopped one"]
    );
}

#[test]
fn wait_until_checks_again_when_state_changes() {
    let (waiting, ready) = mpsc::channel();
    let harness = serve(move |peer| {
        ready.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player1": {"intHP": 700}}}),
        ));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { player, script, wait } from "mira";
        export default function* () {
          yield* wait.until(() => true);
          script.log("waiting");
          yield* wait.until(() => player.hp < 900);
          script.log(player.hp);
        }
        "#,
    );
    let (outcome, outputs) = run_in(
        &harness,
        &path,
        &Cancel::default(),
        signal_on("waiting", waiting),
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["waiting", "700"]);
}

#[test]
fn packets_reach_handlers_and_once_waiters_after_a_raw_send() {
    let (sent, ready) = mpsc::channel();
    let harness = serve(move |peer| {
        peer.expect("%xt%zm%ping%1%");
        ready.recv().unwrap();
        peer.send("%xt%server%-1%pong%");
        peer.send(&json_message(json!({"cmd": "custom", "n": 1})));
        peer.send(&json_message(json!({"cmd": "custom", "n": 2})));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { packets, script } from "mira";
        export default function* () {
          packets.on({ kind: "xt", cmd: "server" }, (p) => script.log("xt", p.cmd, p.args.join("|")));
          yield* packets.send("%xt%zm%ping%1%");
          script.log("sent");
          const p = yield* packets.once({ kind: "json", cmd: "custom" }, (p) => (p.data as any).n === 2);
          script.log("json", p.cmd, (p.data as any).n);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), signal_on("sent", sent));
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["sent", "xt server pong", "json custom 2"]);
}

#[test]
fn settings_apply_while_the_script_runs_and_revert_after() {
    let harness = serve(|_| {});
    let seen = Arc::new(Mutex::new(None));
    let during = Arc::clone(&seen);
    let session = harness.session.clone();
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { script, settings, wait } from "mira";
        export default function* () {
          settings.counterHold = "all";
          settings.antiAfk = true;
          settings.loadCells = true;
          settings.aggro = "cell";
          script.log(settings.counterHold, settings.antiAfk, settings.aggro, settings.room, settings.loadCells);
          for (const bad of [() => (settings.counterHold = "sometimes" as any), () => (settings.room = 1.5)]) {
            try {
              bad();
            } catch (e) {
              script.log((e as Error).name);
            }
          }
          yield* wait.ms(50);
          script.log("check");
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), move |out| {
        if matches!(out, Output::Log { text, .. } if text == "check") {
            *during.lock().unwrap() = Some(session.snapshot().settings.clone());
        }
    });
    thread::sleep(Duration::from_millis(50));
    let after = harness.session.snapshot().settings.clone();
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "all true cell null true",
            "TypeError",
            "RangeError",
            "check"
        ]
    );
    let during = seen.lock().unwrap().clone().unwrap();
    assert_eq!(during.counter_hold, mira_client::CounterHold::All);
    assert_eq!(during.aggro, Some(mira_client::Aggro::Cell));
    assert!(during.anti_afk);
    assert!(during.load_cells);
    assert_eq!(after, mira_client::Settings::default());
}

#[test]
fn a_join_without_a_room_uses_settings_room() {
    let arrive = |peer: &mut Peer, area: i64, name: &str| {
        peer.send(&json_message(json!({
            "cmd": "moveToArea", "areaId": area, "areaName": name,
            "uoBranch": [{"entID": 7, "strUsername": "player1", "strFrame": "Enter"}],
        })));
        peer.send(&format!("%xt%server%-1%You joined \"{name}\"!%"));
        peer.expect(&packet::move_to_cell(AreaId(area), "Enter", "Spawn"));
    };
    let harness = serve(move |peer| {
        let random = peer.recv();
        let room = random
            .strip_prefix("%xt%zm%cmd%1%tfer%player1%yulgar-")
            .and_then(|rest| rest.strip_suffix('%'))
            .and_then(|room| room.parse::<u32>().ok());
        assert!(
            room.is_some_and(|room| (1000..=99999).contains(&room)),
            "{random}"
        );
        arrive(peer, 43, "yulgar-5000");
        peer.expect(&packet::transfer("player1", "farm-1234"));
        arrive(peer, 44, "farm-1234");
        peer.expect(&packet::transfer("player1", "battleon-1"));
        arrive(peer, 45, "battleon-1");
    });
    let (outcome, outputs) = run_on(
        harness,
        r#"
        import { map, script, settings } from "mira";
        export default function* () {
          script.log(yield* map.join("yulgar"), map.name);
          script.log(yield* map.join("Yulgar"), map.name);
          settings.room = 1234;
          script.log(yield* map.join("farm"), map.name);
          settings.room = 1;
          script.log(yield* map.join("battleon"), map.name);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "joined yulgar-5000",
            "joined yulgar-5000",
            "joined farm-1234",
            "joined battleon-1"
        ]
    );
}

#[test]
fn registered_drops_are_picked_up() {
    let harness = serve(|peer| {
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {
            "101": {"ItemID": 101, "sName": "Health Potion", "iQty": 2, "iStk": 10,
                    "sType": "Item", "sES": "None"},
        }})));
        peer.expect(&packet::get_drop(AreaId(42), ItemId(101)));
        peer.send(&json_message(json!({
            "cmd": "getDrop", "bSuccess": 1, "ItemID": 101, "CharItemID": 5001,
            "iQty": 2, "iQtyNow": 7, "bBank": 0,
        })));
    });
    let (outcome, outputs) = run_on(
        harness,
        r#"
        import { drops, inventory, script, wait } from "mira";
        export default function* () {
          drops.register("health potion");
          script.log(drops.registered);
          yield* wait.until(() => inventory.count(101) === 7);
          drops.unregister("health potion");
          script.log("picked", drops.registered.length);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), [r#"["health potion"]"#, "picked 0"]);
}

#[test]
fn registered_quests_turn_in_and_accept_again() {
    let harness = serve(|peer| {
        peer.expect(&packet::get_quests(AreaId(42), &[QuestId(30)]));
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {"30": {
            "QuestID": 30, "sName": "Potion Run",
            "turnin": [{"ItemID": 101, "iQty": 2}],
            "oItems": {"101": {"ItemID": 101, "sName": "Health Potion"}},
        }}})));
        peer.expect(&packet::accept_quest(AreaId(42), QuestId(30)));
        peer.send(&json_message(
            json!({"cmd": "acceptQuest", "QuestID": 30, "bSuccess": 1, "msg": "success"}),
        ));
        peer.expect(&packet::try_quest_complete(
            AreaId(42),
            QuestId(30),
            None,
            1,
        ));
        peer.send(&json_message(json!({"cmd": "turnIn", "sItems": "101:2"})));
        peer.send(&json_message(
            json!({"cmd": "ccqr", "QuestID": 30, "bSuccess": 1, "sName": "Potion Run"}),
        ));
        peer.expect(&packet::accept_quest(AreaId(42), QuestId(30)));
        peer.send(&json_message(
            json!({"cmd": "acceptQuest", "QuestID": 30, "bSuccess": 1, "msg": "success"}),
        ));
    });
    let (outcome, outputs) = run_on(
        harness,
        r#"
        import { events, quests, script } from "mira";
        export default function* () {
          quests.register(30);
          yield* events.once("questCompleted");
          yield* events.once("questAccepted");
          script.log(JSON.stringify(quests.registered));
          quests.unregister(30);
          script.log(quests.registered.length);
        }
        "#,
    );
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), [r#"[{"id":30}]"#, "0"]);
}

struct Relogins(Mutex<Vec<Harness>>);

impl Account for Relogins {
    fn servers(&self) -> Result<Vec<Server>, String> {
        Ok(vec![Server {
            name: "Twilly".into(),
            host: "127.0.0.1".into(),
            port: 1,
            players: 5,
            capacity: 10,
            online: true,
            members_only: false,
        }])
    }

    fn login(&self, server: &str) -> Result<Handle, String> {
        if server == "Full" {
            return Err("Full is full".into());
        }
        let harness = serve(|_| {});
        let session = harness.session.clone();
        self.0.lock().unwrap().push(harness);
        Ok(session)
    }
}

#[test]
fn auth_lists_servers_and_relogs_without_ending_the_run() {
    let harness = serve(|_| {});
    let relogins = Arc::new(Relogins(Mutex::new(Vec::new())));
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { auth, map, player, script, settings } from "mira";
        export default function* () {
          const [server] = yield* auth.servers();
          script.log(server.name, server.players, server.capacity, server.members);
          script.log(auth.server);
          settings.counterHold = "all";
          try {
            yield* auth.relogin("Full");
          } catch (e) {
            script.log((e as Error).message);
          }
          try {
            yield* player.rest();
          } catch (e) {
            script.log((e as Error).message);
          }
          yield* auth.relogin("Other");
          script.log(auth.server, map.name);
        }
        "#,
    );
    let account: Arc<dyn Account> = relogins.clone();
    let seen = Arc::new(Mutex::new(None));
    let during = Arc::clone(&seen);
    let watched = Arc::clone(&relogins);
    let (outcome, outputs) = run_with(
        &harness,
        &path,
        &Cancel::default(),
        &[],
        account,
        move |out| {
            if matches!(out, Output::Log { text, .. } if text.starts_with("Other")) {
                let session = watched.0.lock().unwrap()[0].session.clone();
                let deadline = Instant::now() + Duration::from_secs(2);
                while session.snapshot().settings.counter_hold != mira_client::CounterHold::All
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(1));
                }
                *during.lock().unwrap() = Some(session.snapshot().settings.counter_hold);
            }
        },
    );
    harness.finish();
    for relogged in relogins.0.lock().unwrap().drain(..) {
        finish(relogged);
    }
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "Twilly 5 10 false",
            "Test",
            "couldn't log back in: Full is full",
            "the session is logged out",
            "Other battleon-1"
        ]
    );
    assert_eq!(*seen.lock().unwrap(), Some(mira_client::CounterHold::All));
}

#[test]
fn logout_restart_and_auto_relogin_end_the_run_their_own_way() {
    let ended = |source: &str| {
        let harness = serve(|_| {});
        let dir = Dir::new();
        let path = dir.file("main.ts", source);
        let session = harness.session.clone();
        let closer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            session.logout();
        });
        let (outcome, _) = run_in(&harness, &path, &Cancel::default(), |_| {});
        closer.join().unwrap();
        harness.finish();
        outcome
    };
    assert_eq!(
        ended(
            r#"import { auth } from "mira"; export default function* () { yield* auth.logout(); }"#
        ),
        Ok(Outcome::LoggedOut)
    );
    assert_eq!(
        ended(
            r#"import { script } from "mira"; export default function* () { script.restart(); }"#
        ),
        Ok(Outcome::Restart)
    );
    assert_eq!(
        ended(
            r#"import { settings, wait } from "mira";
            export default function* () {
              settings.autoRelogin = { delayMs: 1500, server: "Twilly" };
              yield* wait.ms(60_000);
            }"#
        ),
        Ok(Outcome::Relogin(mira_script::AutoRelogin {
            delay: Duration::from_millis(1500),
            attempts: 5,
            server: Some("Twilly".into()),
        }))
    );
}

#[test]
fn script_args_are_read_from_the_type_and_parsed_from_text() {
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { script as s } from "mira";
        export default function* () {
          const args = s.args<{
            monster: string;
            count?: number;
            loud: boolean | undefined;
            names: readonly string[];
            weights: Readonly<Record<string, number>>;
          }>({ count: 3, loud: false });
          s.log(JSON.stringify(args));
        }
        "#,
    );
    let script = Script::load(&path, |_| {}, &Cancel::default()).unwrap();
    let names: Vec<(&str, bool)> = script
        .args()
        .iter()
        .map(|arg| (arg.name.as_str(), arg.optional))
        .collect();
    assert_eq!(
        names,
        [
            ("monster", false),
            ("count", true),
            ("loud", true),
            ("names", false),
            ("weights", false)
        ]
    );
    let parse = |given: &[(&str, &str)]| {
        let given: Vec<(String, String)> = given
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        script.parse_args(&given)
    };
    assert_eq!(
        parse(&[("monster", "Treeant")]).unwrap_err(),
        "the script needs --arg for names, weights"
    );
    assert_eq!(
        parse(&[("count", "lots")]).unwrap_err(),
        "count must be number, not lots"
    );
    assert_eq!(
        parse(&[("color", "red")]).unwrap_err(),
        "the script has no arg color; its args are monster, count, loud, names, weights"
    );
    drop(script);

    let harness = serve(|_| {});
    let given = [
        ("monster", "Treeant"),
        ("names", r#"["a","b"]"#),
        ("weights", r#"{"a":1}"#),
        ("loud", "true"),
    ];
    let (outcome, outputs) = run_with(
        &harness,
        &path,
        &Cancel::default(),
        &given,
        Arc::new(NoAccount),
        |_| {},
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [r#"{"count":3,"loud":true,"monster":"Treeant","names":["a","b"],"weights":{"a":1}}"#]
    );
}

#[test]
fn too_many_pending_actions_end_the_run() {
    let (outcome, _) = run_script(
        r#"
        import { player, wait } from "mira";
        export default function* () {
          const rests: Record<string, any> = {};
          for (let i = 0; i <= 100; i++) rests[`r${i}`] = player.rest();
          yield* wait.first(rests);
        }
        "#,
    );
    assert!(failure(outcome).starts_with("MiraError: more than 100 actions are pending"));
    let (outcome, _) = run_script(
        r#"
        import { auth, wait } from "mira";
        export default function* () {
          const lists: Record<string, any> = {};
          for (let i = 0; i <= 100; i++) lists[`s${i}`] = auth.servers();
          yield* wait.first(lists);
        }
        "#,
    );
    assert!(failure(outcome).starts_with("MiraError: more than 100 actions are pending"));
}

#[test]
fn too_many_pending_handler_calls_end_the_run() {
    let (listening, ready) = mpsc::channel();
    let harness = serve(move |peer| {
        ready.recv().unwrap();
        for i in 0..=100 {
            peer.send(&chat(&i.to_string()));
        }
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { events, script, wait } from "mira";
        export default function* () {
          events.on("chat", function* () {
            yield* wait.ms(60_000);
          });
          script.log("listening");
          yield* wait.ms(60_000);
        }
        "#,
    );
    let (outcome, _) = run_in(
        &harness,
        &path,
        &Cancel::default(),
        signal_on("listening", listening),
    );
    finish(harness);
    assert!(failure(outcome).starts_with("MiraError: more than 100 handler calls are pending"));
}

#[test]
fn kill_waits_for_a_respawn_and_uses_the_chosen_skills() {
    let (waiting, ready) = mpsc::channel();
    let (aimed, aims) = mpsc::channel();
    let harness = farm(move |peer| {
        peer.send(&json_message(json!({"cmd": "ct", "m": {
            "1": {"intHP": 0, "intState": 0},
            "2": {"intHP": 0, "intState": 0},
        }})));
        ready.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "m": {"2": {"intHP": 200, "intState": 1}}}),
        ));
        aimed.send(peer.recv()).unwrap();
        peer.answer(0, 1);
        aimed.send(peer.recv()).unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "m": {"2": {"intHP": 0, "intState": 0}}}),
        ));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { combat, map, script, wait } from "mira";
        export default function* () {
          yield* wait.until(() => map.monsters.filter((m) => m.name === "Mosquito").every((m) => m.state === "dead"));
          script.log(yield* combat.kill("Frogzard", { waitForRespawn: true }));
          script.log("waiting");
          script.log(yield* combat.kill("Mosquito", { waitForRespawn: true, skills: [2, 1] }));
        }
        "#,
    );
    let (outcome, outputs) = run_in(
        &harness,
        &path,
        &Cancel::default(),
        signal_on("waiting", waiting),
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(logs(&outputs), ["none", "waiting", "killed"]);
    let sent: Vec<String> = aims.try_iter().collect();
    assert_eq!(
        sent,
        [
            packet::gar(0, "a2", &[Aim::Player(EntityId(7))]),
            packet::gar(1, "a1", &[Aim::Monster(MonMapId(2))]),
        ]
    );
}

#[test]
fn the_floor_zone_follows_the_boss() {
    let (signal, signals) = mpsc::channel::<()>();
    let harness = serve(move |peer| {
        let zone = |zone: &str| json_message(json!({"cmd": "event", "args": {"zoneSet": zone}}));
        signals.recv().unwrap();
        peer.send(&zone("A"));
        peer.send(&zone("A"));
        signals.recv().unwrap();
        peer.send(&zone(""));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { events, map, script, wait } from "mira";
        export default function* () {
          events.on("zone", (e) => script.log("zone", e.zone));
          script.log("listening", map.zone);
          yield* wait.until(() => map.zone === "A");
          script.log("marked", map.zone);
          yield* events.once("zone", (e) => e.zone === null);
          script.log("cleared", map.zone);
        }
        "#,
    );
    let (outcome, outputs) = run_in(&harness, &path, &Cancel::default(), move |out| {
        if matches!(out, Output::Log { text, .. } if text.starts_with("listening") || text.starts_with("marked"))
        {
            let _ = signal.send(());
        }
    });
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "listening null",
            "zone A",
            "marked A",
            "zone null",
            "cleared null"
        ]
    );
}

#[test]
fn what_mira_builds_matches_the_generated_types() {
    let (listening, ready) = mpsc::channel();
    let harness = farm(move |peer| {
        ready.recv().unwrap();
        peer.send(
            "<msg t='sys'><body action='uER' r='43'><u i='10' m='0' s='0' p='3'>\
             <n><![CDATA[player4]]></n><vars></vars></u></body></msg>",
        );
        peer.expect(&packet::retrieve_user_data(AreaId(43), EntityId(10)));
        let aura = |caster: &str, target: &str, name: &str| {
            json_message(json!({"cmd": "aura+", "cInf": caster, "tInf": target,
                "auras": [{"nam": name, "dur": 5, "isNew": true}]}))
        };
        peer.send(&aura("p:7", "m:1", "Bleed"));
        peer.send(&aura("m:1", "p:7", "Stun"));
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"201": {
            "ItemID": 201, "sName": "Frog Leg", "iQty": 1, "iStk": 5, "bTemp": true,
        }}})));
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {"300": {
            "ItemID": 300, "sName": "Mace", "iQty": 1, "iStk": 1, "sType": "Mace", "sES": "Weapon",
        }}})));
        peer.send(&chat("hello"));
        peer.expect(&packet::get_quests(AreaId(43), &[QuestId(30)]));
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {"30": {
            "QuestID": 30, "sName": "Potion Run",
            "turnin": [{"ItemID": 101, "iQty": 2}],
            "oItems": {"101": {"ItemID": 101, "sName": "Health Potion"}},
            "reward": [{"ItemID": 600, "iQty": 1, "iRate": 100, "iType": 2}],
        }}})));
        peer.expect(&packet::load_shop(AreaId(43), ShopId(7)));
        peer.send(&json_message(json!({"cmd": "loadShop", "shopinfo": {
            "ShopID": 7, "sName": "Mixed", "Location": "farm", "items": [
                {"ItemID": 905, "ShopItemID": 74, "sName": "Token Buy", "iCost": 0, "iStk": 10,
                 "iQty": 1, "TokID": 101, "TokQty": 2},
            ],
        }})));
    });
    let dir = Dir::new();
    let path = dir.file(
        "main.ts",
        r#"
        import { auth, bank, combat, drops, events, inventory, map, player, quests, script, shop, wait } from "mira";
        export default function* () {
          const seen: [string, unknown][] = [];
          for (const name of ["playerEntered", "auraAdded", "itemChanged", "dropOffered", "chat"] as const) {
            events.on(name, (event) => {
              seen.push([name, event]);
            });
          }
          script.log("listening");
          yield* wait.until(() => seen.some(([name]) => name === "chat"));
          yield* quests.load(30);
          yield* shop.load(7);
          const servers = yield* auth.servers();
          const { registered, ...pending } = drops;
          script.log(JSON.stringify({
            player, map, combat, inventory, bank, drops: pending, shop,
            quest: quests.get(30), servers, seen,
          }));
        }
        "#,
    );
    let account: Arc<dyn Account> = Arc::new(Relogins(Mutex::new(Vec::new())));
    let (outcome, outputs) = run_with(
        &harness,
        &path,
        &Cancel::default(),
        &[],
        account,
        signal_on("listening", listening),
    );
    finish(harness);
    assert_eq!(outcome, Ok(Outcome::Finished));
    let built: Value = serde_json::from_str(logs(&outputs).last().unwrap()).unwrap();
    for list in [
        "/player/auras",
        "/player/factions",
        "/map/players",
        "/map/monsters",
        "/map/monsters/0/auras",
        "/combat/skills",
        "/inventory/items",
        "/inventory/temp",
        "/drops/pending",
        "/shop/current/items",
        "/quest/turnIn",
        "/quest/rewards",
        "/servers",
    ] {
        let found = built.pointer(list).and_then(Value::as_array);
        assert!(
            found.is_some_and(|items| !items.is_empty()),
            "{list} is empty, so its type went unchecked"
        );
    }

    let api = api_types::Api::load();
    for (key, ty) in [
        ("player", "PlayerFields"),
        ("map", "MapFields"),
        ("combat", "CombatFields"),
        ("inventory", "InventoryFields"),
        ("bank", "BankFields"),
        ("drops", "DropsFields"),
        ("shop", "ShopFields"),
        ("quest", "Quest"),
        ("servers", "readonly Server[]"),
    ] {
        api.check(ty, &built[key]).unwrap();
    }
    let mut events = Vec::new();
    for pair in built["seen"].as_array().unwrap() {
        let name = pair[0].as_str().unwrap();
        api.check_event(name, &pair[1]).unwrap();
        events.push(name);
    }
    events.dedup();
    assert_eq!(
        events,
        [
            "playerEntered",
            "auraAdded",
            "itemChanged",
            "dropOffered",
            "chat"
        ]
    );
}

#[test]
fn map_cells_load_explicitly_return_labels_and_report_http_failures() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    let http = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let base = format!("http://{}/game/api", http.local_addr().unwrap());
    let requests = thread::spawn(move || {
        for (file, cells) in [
            ("forest.swf", Some(vec!["Enter", "r2", "Blank"])),
            ("cave.swf", None),
            ("cave.swf", Some(vec![])),
        ] {
            let (mut stream, _) = http.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                line.trim(),
                format!("GET /game/gamefiles/maps/{file} HTTP/1.1")
            );
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
            }
            if let Some(cells) = cells {
                let body = mira_client::testing::map_swf(&cells);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            } else {
                stream
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n")
                    .unwrap();
            }
        }
    });
    let harness = Harness::start_with_api(Options::default(), base, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready();
        for (area, name, file) in [
            (43, "forest-1", "forest.swf"),
            (44, "forest-2", "forest.swf"),
            (45, "cave-1", "cave.swf"),
        ] {
            peer.expect(&packet::transfer("player1", name));
            let mut message: Value = serde_json::from_str(&move_to_area(area, name)).unwrap();
            message["b"]["o"]["strMapFileName"] = json!(file);
            peer.send(&message.to_string());
            peer.send(&format!("%xt%server%-1%You joined \"{name}\"!%"));
            peer.expect(&packet::move_to_cell(AreaId(area), "Enter", "Spawn"));
            peer.expect(&packet::retrieve_user_datas(AreaId(area), &[EntityId(8)]));
        }
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    let (outcome, outputs) = run_on(
        harness,
        r#"
        import { map, script, settings } from "mira";
        export default function* () {
          script.log(settings.loadCells, map.cells.length);
          yield* map.join("forest-1");
          script.log(map.cells.length);
          const cells: readonly string[] = yield* map.loadCells();
          script.log(cells.join(","), map.cells.join(","));
          yield* map.join("forest-2");
          script.log(map.cells.join(","));
          script.log((yield* map.loadCells()).join(","));
          yield* map.join("cave-1");
          script.log(map.cells.length);
          try { yield* map.loadCells(); }
          catch (e) { script.log(String(e).includes("503"), map.cells.length); }
          script.log((yield* map.loadCells()).length, map.cells.length);
          script.log((yield* map.loadCells()).length);
        }
    "#,
    );
    requests.join().unwrap();
    assert_eq!(outcome, Ok(Outcome::Finished));
    assert_eq!(
        logs(&outputs),
        [
            "false 0",
            "0",
            "Enter,r2,Blank Enter,r2,Blank",
            "Enter,r2,Blank",
            "Enter,r2,Blank",
            "0",
            "true 0",
            "0 0",
            "0"
        ]
    );
}
