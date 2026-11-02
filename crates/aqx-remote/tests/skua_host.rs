#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use aqx_core::{Event, Outcome, Sink};
use server::{FARM, Fake, Wire};

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

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn the_skua_host_runs_a_script_against_the_session() {
    let host = std::env::var_os("AQX_SKUA").expect("AQX_SKUA names the aqx-skua executable");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-host");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("Hello.cs");
    std::fs::write(&script, HELLO).unwrap();

    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink: Sink = {
        let logged = logged.clone();
        Rc::new(move |event: &Event| {
            if let Event::ScriptLog { text, .. } = event {
                logged.lock().unwrap().push(text.clone());
            }
        })
    };
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let mut command = Command::new(host);
    command.arg("serve").arg(&script);
    let result = aqx_remote::run(&mut fake.session, command);

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert_eq!(
        logged,
        [
            "version unknown",
            "tries 5, default delay 800, exit False",
            "Question: Continue? (dismissed, as if the window were closed)",
        ]
    );
}

fn host(script: &Path, home: &Path) -> Command {
    let host = std::env::var_os("AQX_SKUA").expect("AQX_SKUA names the aqx-skua executable");
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skua/vendor/Scripts");
    let mut command = Command::new(host);
    command.env("AQX_SKUA_DATA", home.join("Skua")).arg("serve").arg(script).arg("--scripts").arg(scripts);
    command
}

fn logging() -> (Sink, Arc<Mutex<Vec<String>>>) {
    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink: Sink = {
        let logged = logged.clone();
        Rc::new(move |event: &Event| match event {
            Event::ScriptLog { level, text } => logged.lock().unwrap().push(format!("{level}: {text}")),
            Event::Status { text } => logged.lock().unwrap().push(format!("status: {text}")),
            _ => {}
        })
    };
    (sink, logged)
}

const SET_OPTIONS: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class SetOptionsOnly
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.Logger("options set");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_sets_its_options_up_and_down() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-set-options");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("SetOptionsOnly.cs");
    std::fs::write(&script, SET_OPTIONS).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.contains("options set")), "{logged:#?}");
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}");
    }
}

const HUNT: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class HuntForQuest
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.EnsureAccept(11);
        Core.HuntMonster("farm", "Treeant", "Treeant Branch", 1, isTemp: false);
        Core.EnsureComplete(11);
        Core.Logger($"holding {Bot.Inventory.GetQuantity("Twilly Twig")} Twilly Twig");
        Core.SetOptions(false);
    }
}
"#;

const QUEST_11: &str = r#"{"cmd":"getQuests","quests":{"11":{"QuestID":11,"sName":"Twilly's New Staff","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
    "turnin":[{"ItemID":87,"iQty":1}],"oItems":{"87":{"ItemID":87,"sName":"Treeant Branch","iStk":10}},
    "oRewards":{"itemsS":{"0":{"ItemID":1027,"sName":"Twilly Twig","iStk":1}}},"reward":[{"ItemID":1027,"iQty":1,"iRate":100,"iType":0}]}}}"#;

fn farm_quest_server(msg: &str, wire: &mut Wire) {
    if msg.contains("%getQuests%") {
        wire.object(QUEST_11);
    } else if msg.starts_with("%xt%zm%acceptQuest%5%11%") {
        wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#);
    } else if msg.contains("%gar%") && msg.contains(">m:2") {
        wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
        wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
        wire.object(r#"{"cmd":"dropItem","items":{"87":{"ItemID":87,"sName":"Treeant Branch","iQty":1,"iStk":10}}}"#);
    } else if msg.starts_with("%xt%zm%getDrop%5%87%") {
        wire.object(r#"{"cmd":"getDrop","bSuccess":1,"ItemID":87,"CharItemID":9,"iQty":1,"iQtyNow":1,"bBank":0}"#);
    } else if msg.starts_with("%xt%zm%tryQuestComplete%5%11%") {
        wire.object(r#"{"cmd":"turnIn","sItems":"87:1"}"#);
        wire.object(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":11,"sName":"Twilly's New Staff","rewardObj":{"intGold":100}}"#);
        wire.object(r#"{"cmd":"addItems","items":{"1027":{"ItemID":1027,"sName":"Twilly Twig","iQty":1,"iStk":1,"CharItemID":10}}}"#);
    }
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_hunts_a_monster_for_a_quest_item_and_turns_it_in() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-hunt");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("HuntForQuest.cs");
    std::fs::write(&script, HUNT).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, farm_quest_server, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    let at = |needle: &str| sent.iter().position(|m| m.contains(needle)).unwrap_or_else(|| panic!("never sent {needle}: {sent:#?}\n{logged:#?}"));
    assert!(at("%acceptQuest%5%11%") < at("%moveToCell%5%r2%"));
    assert!(at("%moveToCell%5%r2%") < at(">m:2"));
    assert!(at(">m:2") < at("%getDrop%5%87%"));
    assert!(at("%getDrop%5%87%") < at("%tryQuestComplete%5%11%"));
    assert!(logged.iter().any(|l| l.contains("holding 1 Twilly Twig")), "{logged:#?}");
}
