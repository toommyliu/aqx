mod support;

use mira_script::skua::{SkuaScript, worker};
use mira_script::{Cancel, Outcome};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use support::{Dir, Fake};

const HELLO: &str = r#"
using Skua.Core.Interfaces;

public class Hello
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Log($"version {bot.Version?.ToString() ?? "unknown"}");
        bot.Options.JoinMapTries = 5;
        bot.Log($"tries {bot.Options.JoinMapTries}, default delay {bot.Options.ActionDelay}, exit {bot.ShouldExit}");
        bot.ShowMessageBox("Continue?", "Question", true);
        bot.Sleep(10);
    }
}
"#;

const STATS: &str = r#"
using Skua.Core.Interfaces;

public class ReadStats
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Map.Jump("r2", "Left");
        for (int i = 0; i < 50 && (bot.Player.Stats?.CriticalMultiplier ?? 0) == 0; i++)
            bot.Sleep(20);
        var s = bot.Player.Stats!;
        bot.Log($"{s.Strength} {s.AttackPower} {s.SpellPower} {s.CriticalChance} {s.CriticalMultiplier} {s.Haste}");
        bot.Log($"coins {bot.Flash.GetGameObject<int>("world.myAvatar.objData.intCoins")}");
        bot.Log($"id {bot.Player.ID}, in r2: {string.Join(", ", bot.Map.CellPlayers!.Select(p => $"{p.Name} {p.EntID}"))}");
        var monsters = Newtonsoft.Json.Linq.JArray.Parse(bot.Flash.Call("availableMonsters")!);
        bot.Log($"monsters in r2: {string.Join(", ", monsters.Select(m => $"{m["strMonName"]} {m["MonMapID"]} {m["intHP"]}"))}");
    }
}
"#;

const STOPS: &str = r#"
using Skua.Core.Interfaces;

public class Stops
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Events.ScriptStopping += e =>
        {
            bot.Sleep(250);
            bot.Log($"stopping handler finished, crashed {e is not null}");
            return true;
        };
        if ("MODE" == "finish")
            return;
        bot.StopAsync(true).GetAwaiter().GetResult();
        bot.Map.Jump("r2", "Left");
    }
}
"#;

const IN_PLACE: &str = r#"
using Skua.Core.Interfaces;

public class InPlace
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Map.Jump("Enter", "Spawn");
        bot.Map.Join("farm-99999", "r2", "Left");
        bot.Log($"last map {bot.Map.LastMap}, counter handler {bot.Combat.EnableCounterHandler}");
    }
}
"#;

#[test]
#[ignore = "run with npm run skua:test"]
fn the_skua_host_runs_a_script_against_the_session() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(HELLO);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert_eq!(
        logs,
        [
            "version unknown",
            "tries 5, default delay 800, exit False",
            "Question: Continue? (dismissed, as if the window were closed)"
        ]
    );
}
#[test]
#[ignore = "run with npm run skua:test"]
fn a_host_stopped_while_starting_exits_quietly() {
    let dir = Dir::new();
    let path = dir.script(HELLO);
    let mut child = Command::new(worker().unwrap())
        .arg("serve")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("\"call\":\"listen\""), "{line}");
    writeln!(stdin, "{{\"stop\":{{}}}}").unwrap();
    drop(stdin);
    let status = child.wait().unwrap();
    let mut errors = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(
        status.success() && !errors.contains("Unhandled exception"),
        "{status} {errors}"
    );
}
#[test]
#[ignore = "run with npm run skua:test"]
fn stopping_from_the_script_unwinds_it_and_lets_its_stopping_handlers_finish() {
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(&STOPS.replace("MODE", "stop"));
    assert!(
        matches!(outcome, Outcome::Stopped(_)),
        "{outcome:?} {logs:?}"
    );
    assert!(
        !sent.iter().any(|m| m.contains("%moveToCell%5%r2%")),
        "{sent:?}"
    );
    assert!(
        logs.contains(&"stopping handler finished, crashed False".into()),
        "{logs:?}"
    );
}
#[test]
#[ignore = "run with npm run skua:test"]
fn a_script_that_returns_raises_no_script_stopping() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(&STOPS.replace("MODE", "finish"));
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        !logs.iter().any(|s| s.contains("stopping handler")),
        "{logs:?}"
    );
}
#[test]
#[ignore = "run with npm run skua:test"]
fn a_script_reads_the_players_id_cell_and_combat_stats() {
    let (outcome, logs, _)=Fake::start(|message,wire| if message.contains("%moveToCell%") {
        wire.object(r#"{"cmd":"balance","intGold":10,"intCoins":50}"#);
        wire.object(r#"{"cmd":"stu","sta":{"$STR":7,"$ap":14,"$sp":37,"$tcr":0.08,"$scm":1.66,"$tha":0.03}}"#);
    }).run(STATS);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "7 14 37 0.08 1.66 0.03",
        "coins 50",
        "id 1001, in r2: player1 1001",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert!(
        logs.iter()
            .any(|s| s.starts_with("monsters in r2: Treeant 2 ")),
        "{logs:?}"
    );
}
#[test]
#[ignore = "run with npm run skua:test"]
fn jumping_in_place_still_sends_and_joining_the_current_map_does_nothing() {
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(IN_PLACE);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert_eq!(
        sent.iter()
            .filter(|m| m.starts_with("%xt%zm%moveToCell%5%Enter%Spawn%"))
            .count(),
        2,
        "{sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|m| m.contains("%tfer%") || m.contains("%r2%")),
        "{sent:?}"
    );
    assert!(
        logs.contains(&"last map farm, counter handler False".into()),
        "{logs:?}"
    );
}

#[test]
fn protocol_fixtures_round_trip() {
    use mira_script::skua::protocol::{Message, Request, Worker};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skua/fixtures");
    for file in ["requests", "worker", "messages"] {
        let values: Vec<Value> = serde_json::from_str(
            &std::fs::read_to_string(root.join(format!("{file}.json"))).unwrap(),
        )
        .unwrap();
        for value in values {
            let encoded = match file {
                "requests" => {
                    serde_json::to_value(serde_json::from_value::<Request>(value.clone()).unwrap())
                        .unwrap()
                }
                "worker" => {
                    serde_json::to_value(serde_json::from_value::<Worker>(value.clone()).unwrap())
                        .unwrap()
                }
                _ => {
                    serde_json::to_value(serde_json::from_value::<Message>(value.clone()).unwrap())
                        .unwrap()
                }
            };
            assert_eq!(value, encoded, "{file}");
        }
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn an_event_handler_can_wait_for_an_action_and_reads_its_state() {
    let source = r#"
using Skua.Core.Interfaces;
public class Events {
    bool finished;
    public void ScriptMain(IScriptInterface bot) {
        bot.Events.CellChanged += (map,cell,pad) => {
            if (cell != "r2") return;
            bot.Map.Jump("r3", "Left");
            bot.Log($"handler in {bot.Player.Cell}");
            finished = true;
        };
        bot.Map.Jump("r2", "Left");
        for (int i = 0; i < 100 && !Volatile.Read(ref finished); i++) bot.Sleep(20);
        if (!finished) throw new Exception("the cell handler did not finish");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"handler in r3".into()), "{logs:?}");
    assert!(
        sent.iter().any(|m| m.contains("%moveToCell%5%r3%Left%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_cached_script_does_not_load_the_compiler() {
    let source = r#"
using Skua.Core.Interfaces;
public class Cached {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"compiler loaded {AppDomain.CurrentDomain.GetAssemblies().Any(a => a.GetName().Name!.StartsWith("Microsoft.CodeAnalysis"))}");
    }
}
"#;
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"compiler loaded False".into()), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn cancellation_allows_stopping_handlers_to_act() {
    let cancel = Cancel::default();
    let signal = cancel.clone();
    let source = r#"
using Skua.Core.Interfaces;
public class Cancelled {
    public void ScriptMain(IScriptInterface bot) {
        bot.Events.ScriptStopping += error => { bot.Map.Jump("r2", "Left"); bot.Log("cleanup finished"); return true; };
        bot.Log("ready to stop");
        bot.Sleep(20000);
        bot.Log("still running");
    }
}
"#;
    let (outcome, logs, sent) =
        Fake::start(|_, _| {}).run_with(source, &cancel, Default::default(), move |text| {
            if text == "ready to stop" {
                signal.cancel();
            }
        });
    assert_eq!(outcome, Outcome::Cancelled, "{logs:?}");
    assert!(logs.contains(&"cleanup finished".into()), "{logs:?}");
    assert!(!logs.contains(&"still running".into()), "{logs:?}");
    assert!(
        sent.iter().any(|m| m.contains("%moveToCell%5%r2%Left%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn options_accept_names_and_group_names_without_splitting_values() {
    let source = r#"
using Skua.Core.Interfaces;
using Skua.Core.Options;
public class Configured {
    public string OptionsStorage = "fixture";
    public List<IOption> Options = new() { new Option<string>("Name", "default", "example", transient: true) };
    public string[] MultiOptions = new[] { "Group" };
    public List<IOption> Group = new() { new Option<string>("Other", "default", "example", transient: true) };
    public void ScriptMain(IScriptInterface bot) { bot.Log($"{bot.Config!.Get<string>("Name")} {bot.Config.Get<string>("Group", "Other")}"); }
}
"#;
    let args = [
        ("Name".into(), json!("first=value")),
        ("Group:Other".into(), json!("second:value")),
    ]
    .into_iter()
    .collect();
    let (outcome, logs, _) =
        Fake::start(|_, _| {}).run_with(source, &Cancel::default(), args, |_| {});
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        logs.contains(&"first=value second:value".into()),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn includes_compile_together_and_errors_surface_before_a_session() {
    let dir = Dir::new();
    std::fs::write(
        dir.0.join("CoreBots.cs"),
        "internal class Included { internal static int Value => 7; }",
    )
    .unwrap();
    let path = dir.script(r#"//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;
public class Includes { public void ScriptMain(IScriptInterface bot) { bot.Log(Included.Value.ToString()); } }
"#);
    assert!(SkuaScript::load(&path, |_| {}, &Cancel::default()).is_ok());
    std::fs::write(&path, "public class Broken {").unwrap();
    let error = SkuaScript::load(&path, |_| {}, &Cancel::default())
        .err()
        .unwrap();
    assert!(error.to_string().contains("CS1513"), "{error}");
}
