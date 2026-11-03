#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use aqx_core::{Aura, Enhancement, Event, Item, Outcome, ScriptError, Sink};
use aqx_protocol::{CharItemId, ItemId};
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

const BUY: &str = r#"
//cs_include Scripts/CoreBots.cs
//cs_include Scripts/CoreAdvanced.cs
using Skua.Core.Interfaces;

public class BuyFromShop
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;
    private CoreAdvanced Adv = new();

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.BuyItem("farm", 1348, "Sword");
        Adv.BuyItem("farm", 1348, "Hat");
        Core.Logger($"holding {Bot.Inventory.GetQuantity("Sword")} Sword and {Bot.Inventory.GetQuantity("Hat")} Hat");
        Core.Logger($"shops loaded: {string.Join(", ", Bot.Shops.LoadedCache.Select(s => $"{s.ID} with {s.Items.Count} items"))}");
        Core.SetOptions(false);
    }
}
"#;

fn shop_server(msg: &str, wire: &mut Wire) {
    if msg.contains("%loadShop%5%1348%") {
        wire.object(
            r#"{"cmd":"loadShop","shopinfo":{"ShopID":1348,"sName":"Featured Gear Shop","iIndex":-1,"items":[
            {"ItemID":102658,"ShopItemID":68071,"sName":"Sword","iCost":300,"bCoins":false,"bUpg":false,"iStk":1,"iQty":1,"sType":"Sword","sES":"Weapon","turnin":[]},
            {"ItemID":5,"ShopItemID":6,"sName":"Hat","iCost":100,"bCoins":false,"bUpg":false,"iStk":1,"iQty":1,"sType":"Helm","sES":"he","turnin":[]}]}}"#,
        );
    } else if msg.contains("%buyItem%5%102658%1348%68071%1%") {
        wire.object(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":102658,"CharItemID":77,"iQty":1,"bBank":0}"#);
    } else if msg.contains("%buyItem%5%5%1348%6%1%") {
        wire.object(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":5,"CharItemID":78,"iQty":1,"bBank":0}"#);
    }
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_and_core_advanced_buy_from_a_shop() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-buy");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("BuyFromShop.cs");
    std::fs::write(&script, BUY).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, shop_server, sink);
    let me = fake.session.world.me.as_mut().unwrap();
    (me.gold, me.bag_slots) = (1000, 30);
    fake.session.world.set_bank(Vec::new());
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert_eq!(sent.iter().filter(|m| m.contains("%buyItem%5%102658%1348%68071%1%")).count(), 1, "{sent:#?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.contains("holding 1 Sword and 1 Hat")), "{logged:#?}");
    assert!(logged.iter().any(|l| l.contains("shops loaded: 1348 with 2 items")), "{logged:#?}");
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_dropped_connection_stops_core_bots_without_crashing_the_host() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-hang-up");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("HuntForQuest.cs");
    std::fs::write(&script, HUNT).unwrap();

    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%gar%") {
            wire.hang_up();
        } else {
            farm_quest_server(msg, wire);
        }
    });
    let mut child = host(&script, &home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let (to_host, from_host) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    let result = aqx_remote::serve(&mut fake.session, from_host, to_host);
    let output = child.wait_with_output().unwrap();

    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(matches!(result, Err(ScriptError::Halted(_))), "{result:?}");
    assert!(output.status.success(), "{}\n{errors}", output.status);
}

const BOOST: &str = r#"
using Skua.Core.Interfaces;
using Skua.Core.Models.Items;

public class UseBoost
{
    public IScriptInterface Bot => IScriptInterface.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Bot.Boosts.SetClassBoostID(false);
        Bot.Boosts.UseClassBoost = true;
        Bot.Boosts.Start();
        Bot.Wait.ForTrue(() => Bot.Boosts.IsBoostActive(BoostType.Class), 20);
        Bot.Log($"class boost {Bot.Boosts.ClassBoostID} active: {Bot.Boosts.IsBoostActive(BoostType.Class)}");
        Bot.Boosts.Stop();
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn skua_boosts_use_a_boost_item_until_the_boost_is_running() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-boost");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("UseBoost.cs");
    std::fs::write(&script, BOOST).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%serverUseItem%") && msg.contains("%+%22447%") {
                wire.object(r#"{"cmd":"cpboost","op":"+","iSecsLeft":1200,"bShowShop":false}"#);
            }
        },
        sink,
    );
    fake.session.world.inventory.push(Item {
        id: ItemId(22447),
        char_item: None,
        name: "Daily Login Class Boost  (20 min)".into(),
        qty: 15,
        max_stack: 99,
        kind: "ServerUse".into(),
        slot: "None".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert_eq!(sent.iter().filter(|m| m.contains("%serverUseItem%")).count(), 1, "{sent:#?}");
    assert!(logged.iter().any(|l| l.contains("class boost 22447 active: True")), "{logged:#?}");
}

const PACKETS: &str = r#"
using Skua.Core.Interfaces;

public class ReadPackets
{
    public void ScriptMain(IScriptInterface bot)
    {
        string? json = null, str = null;
        void Read(dynamic packet)
        {
            dynamic data = packet["params"].dataObj;
            if (packet["params"].type == "json")
                json ??= $"{data.cmd} {data.anims[0].msg}";
            else
                str ??= $"{data[0]} {data[1]} {data[2]} {data[3]}";
        }
        string? zone = null;
        bot.Events.ExtensionPacketReceived += Read;
        bot.Events.RunToArea += z => zone = z;
        bot.Map.Jump("r2", "Left");
        for (int i = 0; i < 50 && (json is null || str is null || zone is null); i++)
            bot.Sleep(20);
        bot.Events.ExtensionPacketReceived -= Read;
        bot.Log($"json: {json}");
        bot.Log($"str: {str}");
        bot.Log($"zone: {zone}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_script_reads_raw_packets_and_zone_changes_the_way_skua_hands_them_over() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-packets");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("ReadPackets.cs");
    std::fs::write(&script, PACKETS).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%moveToCell%") {
                wire.object(r#"{"cmd":"ct","anims":[{"msg":"Treeant prepares a counter attack!"}]}"#);
                wire.send("%xt%uotls%-1%hero%afk:true%");
                wire.object(r#"{"cmd":"event","args":{"zoneSet":"B"}}"#);
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: json: ct Treeant prepares a counter attack!".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: str: uotls -1 hero afk:true".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: zone: B".to_string()), "{logged:#?}");
}

const KILL_AND_HUNT: &str = r#"
using Skua.Core.Interfaces;

public class KillAndHunt
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Map.Jump("r2", "Left");
        bot.Kill.Monster("Treeant");
        bot.Log("killed");
        bot.Hunt.ForItem("Treeant", "Treeant Sap", 2, true);
        bot.Log($"holding {bot.TempInv.GetQuantity("Treeant Sap")} Treeant Sap");
    }
}
"#;

fn treeant_drops_sap(msg: &str, wire: &mut Wire) {
    static SAP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if msg.contains("%gar%") && msg.contains(">m:2") {
        wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
        wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
        let now = SAP.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        wire.object(&format!(r#"{{"cmd":"addItems","items":{{"88":{{"ItemID":88,"sName":"Treeant Sap","iQty":1,"iQtyNow":{now},"iStk":10,"bTemp":1}}}}}}"#));
        wire.later(std::time::Duration::from_millis(50), "respawn", r#"{"cmd":"mtls","id":2,"o":{"intHP":540,"intState":1}}"#.into());
    }
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn skua_kills_a_monster_and_hunts_it_for_a_temp_item() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-kill-hunt");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("KillAndHunt.cs");
    std::fs::write(&script, KILL_AND_HUNT).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, treeant_drops_sap, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: killed".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: holding 2 Treeant Sap".to_string()), "{logged:#?}");
}

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

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_script_reads_the_players_id_cell_and_combat_stats() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-stats");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("ReadStats.cs");
    std::fs::write(&script, STATS).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%moveToCell%") {
                wire.object(r#"{"cmd":"balance","intGold":10,"intCoins":50}"#);
                wire.object(r#"{"cmd":"stu","sta":{"$STR":7,"$ap":14,"$sp":37,"$tcr":0.08,"$scm":1.66,"$tha":0.03}}"#);
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: 7 14 37 0.08 1.66 0.03".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: coins 50".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: id 1001, in r2: hero 1001".to_string()), "{logged:#?}");
    assert!(logged.iter().any(|l| l.starts_with("info: monsters in r2: Treeant 2 ")), "{logged:#?}");
}

const LOGOUT: &str = r#"
using Skua.Core.Interfaces;

public class LogOut
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Options.AutoRelogin = false;
        bot.Servers.Logout();
        bot.Sleep(5000);
        bot.Log("still running");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn logging_out_without_auto_relogin_stops_the_script() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-logout");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("LogOut.cs");
    std::fs::write(&script, LOGOUT).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Stopped(_))), "{result:?}\n{logged:#?}");
    assert!(!logged.iter().any(|l| l.contains("still running")), "{logged:#?}");
}

const TARGET_STATE: &str = r#"
using Skua.Core.Interfaces;

public class TargetState
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Map.Jump("r2", "Left");
        bot.Combat.Attack("Treeant");
        for (int i = 0; i < 50 && bot.Player.Target?.State != 2; i++)
            bot.Sleep(20);
        bot.Log($"target {bot.Player.Target?.Name} in state {bot.Player.Target?.State}");
        bot.Combat.CancelTarget();
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_monster_in_combat_reads_as_state_2() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-target-state");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("TargetState.cs");
    std::fs::write(&script, TARGET_STATE).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%gar%") {
                wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":800,"intState":2}}}"#);
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: target Treeant in state 2".to_string()), "{logged:#?}");
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_host_stopped_while_starting_exits_quietly() {
    use std::io::{BufRead, BufReader, Read, Write};
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-stopped-early");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Hello.cs");
    std::fs::write(&script, HELLO).unwrap();

    let mut child = host(&script, &home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut to_host = child.stdin.take().unwrap();
    let mut from_host = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    from_host.read_line(&mut line).unwrap();
    assert!(line.contains(r#""call":"listen""#), "{line}");
    writeln!(to_host, r#"{{"stop":"Stopped"}}"#).unwrap();
    drop(to_host);
    let status = child.wait().unwrap();
    let mut errors = String::new();
    child.stderr.take().unwrap().read_to_string(&mut errors).unwrap();
    assert!(status.success() && !errors.contains("Unhandled exception"), "{status}\n{errors}");
}

const QUEST_REPEATS: &str = r#"
using Skua.Core.Interfaces;

public class QuestRepeats
{
    public void ScriptMain(IScriptInterface bot)
    {
        var quest = bot.Quests.EnsureLoad(12)!;
        bot.Log($"{quest.Name} rewards {string.Join(", ", quest.Rewards.Select(r => r.Name))}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_quest_may_list_the_same_reward_twice() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-quest-repeats");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("QuestRepeats.cs");
    std::fs::write(&script, QUEST_REPEATS).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{"12":{"QuestID":12,"sName":"Twice","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
                    "turnin":[],"oItems":{},"oRewards":{"itemsS":{"0":{"ItemID":1027,"sName":"Twilly Twig","iStk":1},"1":{"ItemID":1027,"sName":"Twilly Twig","iStk":1}}},
                    "reward":[{"ItemID":1027,"iQty":1,"iRate":50,"iType":0},{"ItemID":1027,"iQty":2,"iRate":100,"iType":0}]}}}"#,
                );
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.starts_with("info: Twice rewards Twilly Twig")), "{logged:#?}");
}

const ACCOUNT_AND_POTION: &str = r#"
using System.Dynamic;
using System.Linq;
using Newtonsoft.Json;
using Skua.Core.Interfaces;

public class AccountAndPotion
{
    public void ScriptMain(IScriptInterface bot)
    {
        bool afk = false;
        bot.Events.PlayerAFK += () => afk = true;
        bot.Log($"char {bot.Flash.GetGameObject<int>("world.myAvatar.objData.CharID")}, aq {bot.Flash.GetGameObject<int>("world.myAvatar.objData.intAQ")}, verified {bot.Flash.CallGameFunction<bool>("world.myAvatar.isEmailVerified")}");

        var potion = bot.Inventory.Items.First(i => i.ID == 11857);
        bot.Log($"potion {potion.ItemGroup} {potion.Meta} {bot.Flash.GetGameObject<string>("world.invTree.11857.sLink")} {bot.Flash.GetGameObject<string>("world.invTree.11857.sFile")}");

        dynamic item = new ExpandoObject();
        item.ItemID = potion.ID;
        item.sLink = bot.Flash.GetGameObject<string>($"world.invTree.{potion.ID}.sLink");
        bot.Flash.CallGameFunction("toggleItemEquip", item);
        bot.Log($"equipped {bot.Inventory.IsEquipped(11857)}");

        var skill = bot.Flash.GetArrayObject<dynamic>("world.actions.active", 5);
        bot.Flash.CallGameFunction("world.testAction", JsonConvert.DeserializeObject<ExpandoObject>(JsonConvert.SerializeObject(skill)));

        bot.Map.Jump("r2", "Left");
        for (int i = 0; i < 50 && !afk; i++)
            bot.Sleep(20);
        bot.Log($"afk {afk}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_reads_the_account_equips_and_drinks_a_potion_and_hears_afk() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-account");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("AccountAndPotion.cs");
    std::fs::write(&script, ACCOUNT_AND_POTION).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%geia%") {
                wire.object(r#"{"cmd":"seia","o":{"id":457,"cd":60000,"tgt":"f"},"iRes":1}"#);
            } else if msg.contains("%moveToCell%5%r2%") {
                wire.send("%xt%uotls%-1%hero%afk:true%");
            }
        },
        sink,
    );
    let me = fake.session.world.me.as_mut().unwrap();
    (me.char_id, me.aq_linked, me.email_verified) = (Some(42), true, true);
    fake.session.world.inventory.push(Item {
        id: ItemId(11857),
        char_item: None,
        name: "Health Potion".into(),
        qty: 3,
        max_stack: 10,
        kind: "Item".into(),
        slot: "None".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: "potion".into(),
        file: "items/potion.swf".into(),
        icon: String::new(),
        meta: "457".into(),
        level: 0,
        enhancement: None,
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: char 42, aq 1, verified True".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: potion None 457 potion items/potion.swf".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: equipped True".to_string()), "{logged:#?}");
    assert!(sent.iter().any(|m| m.contains("%geia%") && m.contains("%potion%457%11857%")), "{sent:#?}");
    assert!(sent.iter().any(|m| m.contains("%gar%") && m.contains("i1>")), "{sent:#?}");
    assert!(logged.contains(&"info: afk True".to_string()), "{logged:#?}");
}

const STOPS: &str = r#"
using Skua.Core.Interfaces;

public class Stops
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Events.ScriptStopping += e =>
        {
            bot.Sleep(2500);
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

fn run_stops(mode: &str) -> (Result<Outcome, ScriptError>, Vec<String>, Vec<String>) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("skua-stops-{mode}"));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Stops.cs");
    std::fs::write(&script, STOPS.replace("MODE", mode)).unwrap();
    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent = fake.finish().into_iter().map(|(_, m)| m).collect();
    let logged = logged.lock().unwrap().clone();
    (result, logged, sent)
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn stopping_from_the_script_unwinds_it_and_lets_its_stopping_handlers_finish() {
    let (result, logged, sent) = run_stops("stop");
    assert!(matches!(result, Ok(Outcome::Stopped(_))), "{result:?}\n{logged:#?}");
    assert!(!sent.iter().any(|m| m.contains("%moveToCell%5%r2%")), "{sent:#?}");
    assert!(logged.contains(&"info: stopping handler finished, crashed False".to_string()), "{logged:#?}");
}

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_script_that_returns_raises_no_script_stopping() {
    let (result, logged, _) = run_stops("finish");
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(!logged.iter().any(|l| l.contains("stopping handler")), "{logged:#?}");
}

const CHOICE: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class ChooseReward
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        var sword = Bot.Inventory.Items.First(i => i.ID == 7);
        Core.Logger($"sword {sword.CharItemID} pattern {sword.EnhancementPatternID}");
        Core.EnsureAccept(13);
        Core.Logger($"completed {Core.EnsureComplete(13)}");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_picks_a_choice_reward_and_reads_an_items_char_id_and_pattern() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-choice");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("ChooseReward.cs");
    std::fs::write(&script, CHOICE).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{"13":{"QuestID":13,"sName":"Pick One","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
                    "turnin":[],"oItems":{},"oRewards":{"itemsC":{"0":{"ItemID":1027,"sName":"Twilly Twig","iStk":1},"1":{"ItemID":1028,"sName":"Twilly Leaf","iStk":1}}},
                    "reward":[{"ItemID":1027,"iQty":1,"iRate":100,"iType":2},{"ItemID":1028,"iQty":1,"iRate":100,"iType":2}]}}}"#,
                );
            } else if msg.starts_with("%xt%zm%acceptQuest%5%13%") {
                wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":13,"msg":"success"}"#);
            } else if msg.starts_with("%xt%zm%tryQuestComplete%5%13%") {
                wire.object(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":13,"sName":"Pick One","rewardObj":{"intGold":0}}"#);
                wire.object(r#"{"cmd":"addItems","items":{"1027":{"ItemID":1027,"sName":"Twilly Twig","iQty":1,"iStk":1,"CharItemID":10}}}"#);
            }
        },
        sink,
    );
    fake.session.world.inventory.push(Item {
        id: ItemId(7),
        char_item: Some(CharItemId(77)),
        name: "Sword".into(),
        qty: 1,
        max_stack: 1,
        kind: "Sword".into(),
        slot: "Weapon".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 1,
        enhancement: Some(Enhancement { id: 3719, level: 22, pattern: 2 }),
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.ends_with("sword 77 pattern 2")), "{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%13%1027%")), "{sent:#?}");
    assert!(logged.iter().any(|l| l.ends_with("completed True")), "{logged:#?}");
}

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
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn jumping_in_place_still_sends_and_joining_the_current_map_does_nothing() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-in-place");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("InPlace.cs");
    std::fs::write(&script, IN_PLACE).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert_eq!(sent.iter().filter(|m| m.starts_with("%xt%zm%moveToCell%5%Enter%Spawn%")).count(), 2, "{sent:#?}");
    assert!(!sent.iter().any(|m| m.contains("%tfer%") || m.contains("%r2%")), "{sent:#?}");
    assert!(logged.contains(&"info: last map farm, counter handler False".to_string()), "{logged:#?}");
}

const UPDATE_QUEST: &str = r#"
using Skua.Core.Interfaces;

public class Unlock
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Quests.Load(14, 15);
        bool before = bot.Quests.IsUnlocked(15);
        bot.Quests.UpdateQuest(14);
        bot.Log($"unlocked {before} then {bot.Quests.IsUnlocked(15)}, accepted {bot.Quests.EnsureAccept(15)}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn update_quest_moves_the_quest_slot_the_way_skuas_fake_packet_does() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-update-quest");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Unlock.cs");
    std::fs::write(&script, UPDATE_QUEST).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{
                    "14":{"QuestID":14,"sName":"First","bOnce":true,"iSlot":5,"iValue":3,"bUpg":false,"iLvl":0,"turnin":[],"oItems":{},"oRewards":{},"reward":[]},
                    "15":{"QuestID":15,"sName":"Second","bOnce":true,"iSlot":5,"iValue":4,"bUpg":false,"iLvl":0,"turnin":[],"oItems":{},"oRewards":{},"reward":[]}}}"#,
                );
            } else if msg.starts_with("%xt%zm%acceptQuest%5%15%") {
                wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":15,"msg":"success"}"#);
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: unlocked False then True, accepted True".to_string()), "{logged:#?}");
}

const AVAILABLE: &str = r#"
using Skua.Core.Interfaces;

public class Available
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Log($"available {bot.Quests.IsAvailable(16)}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_quest_whose_held_item_is_missing_is_not_available() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-available");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Available.cs");
    std::fs::write(&script, AVAILABLE).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{"16":{"QuestID":16,"sName":"Badge Holders","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
                    "turnin":[],"oItems":{},"oRewards":{},"reward":[],"reqd":[{"ItemID":900,"iQty":1}],"oReqd":{"900":{"ItemID":900,"sName":"Badge","iStk":1}}}}}"#,
                );
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: available False".to_string()), "{logged:#?}");
}

const REST: &str = r#"
using System.Diagnostics;
using Skua.Core.Interfaces;

public class Rest
{
    public void ScriptMain(IScriptInterface bot)
    {
        Stopwatch clock = Stopwatch.StartNew();
        bot.Player.Rest();
        long quick = clock.ElapsedMilliseconds;
        bot.Player.Rest(true);
        bot.Log($"rest {(quick < 1000 ? "returned" : "blocked")}, full rest gave up {(clock.ElapsedMilliseconds - quick < 5000 ? "after 2 s" : "late")}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn resting_sends_once_and_waits_for_full_only_briefly() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-rest");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Rest.cs");
    std::fs::write(&script, REST).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    fake.session.world.me.as_mut().unwrap().vitals.hp = 100;
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: rest returned, full rest gave up after 2 s".to_string()), "{logged:#?}");
    assert_eq!(sent.iter().filter(|m| m.starts_with("%xt%zm%restRequest%")).count(), 2, "{sent:#?}");
}

const LOAD: &str = r#"
using System.Diagnostics;
using Skua.Core.Interfaces;

public class LoadQuests
{
    public void ScriptMain(IScriptInterface bot)
    {
        Stopwatch clock = Stopwatch.StartNew();
        bot.Quests.Load(11, 404);
        bot.Log($"load {(clock.ElapsedMilliseconds < 1000 ? "returned" : "blocked")}");
        bot.Wait.ForTrue(() => bot.Quests.Tree.Exists(q => q.ID == 11), 20);
        bot.Log($"tree has 11: {bot.Quests.Tree.Exists(q => q.ID == 11)}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn loading_quests_returns_without_waiting_for_ones_the_server_never_sends() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-load");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("LoadQuests.cs");
    std::fs::write(&script, LOAD).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, farm_quest_server, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: load returned".to_string()), "{logged:#?}");
    assert!(logged.contains(&"info: tree has 11: True".to_string()), "{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%getQuests%5%11%404%")), "{sent:#?}");
}

const PICKUP: &str = r#"
using System.Diagnostics;
using Skua.Core.Interfaces;

public class Pickup
{
    public void ScriptMain(IScriptInterface bot)
    {
        Stopwatch clock = Stopwatch.StartNew();
        bot.Drops.Pickup("Treeant Branch");
        bot.Log($"pickup {(clock.ElapsedMilliseconds < 1000 ? "returned" : "blocked")}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn a_pickup_the_server_ignores_does_not_hold_up_the_script() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-pickup");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Pickup.cs");
    std::fs::write(&script, PICKUP).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    fake.session.world.drops.push(Item {
        id: ItemId(87),
        char_item: None,
        name: "Treeant Branch".into(),
        qty: 1,
        max_stack: 10,
        kind: "Item".into(),
        slot: "None".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: pickup returned".to_string()), "{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%getDrop%5%87%")), "{sent:#?}");
}

const MISC_POOL: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class MiscPool
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        var dust = Bot.Inventory.Items.First(i => i.ID == 5);
        var newResource = new Skua.Core.Models.Items.InventoryItem { ID = 6, Name = "Ash", CategoryString = "Resource" };
        var newSword = new Skua.Core.Models.Items.InventoryItem { ID = 7, Name = "Blade", CategoryString = "Sword" };
        bot.Log($"misc {Core.InventoryMiscUsedSlots}/{Core.InventoryMiscSlots}, room for more dust {Core.HasSpaceFor(dust)}, a new resource {Core.HasSpaceFor(newResource)}, a sword {Core.HasSpaceFor(newSword)}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_sees_a_full_misc_pool_through_inv_cat() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-misc-pool");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("MiscPool.cs");
    std::fs::write(&script, MISC_POOL).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    fake.session.world.misc_slots = 1;
    fake.session.world.me.as_mut().unwrap().bag_slots = 10;
    fake.session.world.inventory.push(Item {
        id: ItemId(5),
        char_item: Some(CharItemId(15)),
        name: "Bone Dust".into(),
        qty: 3,
        max_stack: 50,
        kind: "Resource".into(),
        slot: "None".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: misc 1/1, room for more dust True, a new resource False, a sword True".to_string()), "{logged:#?}");
}

const BACKGROUND_OPTIONS: &str = r#"
using Skua.Core.Interfaces;

public class BackgroundOptions
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Options.RestPackets = true;
        bot.Options.AggroAllMonsters = true;
        bot.Sleep(1500);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn rest_packets_and_aggro_all_monsters_send_in_the_background() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-background-options");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("BackgroundOptions.cs");
    std::fs::write(&script, BACKGROUND_OPTIONS).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%restRequest%")), "{sent:#?}");
    assert!(sent.iter().filter(|m| m.starts_with("%xt%zm%aggroMon%5%1%2%")).count() >= 3, "{sent:#?}");
}

const FULL_CHECK: &str = r#"
using Skua.Core.Interfaces;

public class FullCheck
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Log($"can complete {bot.Quests.CanComplete(11)}, counting the bank {bot.Quests.CanCompleteFullCheck(11)}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn the_full_completion_check_counts_the_bank() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-full-check");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("FullCheck.cs");
    std::fs::write(&script, FULL_CHECK).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, farm_quest_server, sink);
    fake.session.world.set_bank(vec![Item {
        id: ItemId(87),
        char_item: Some(CharItemId(9)),
        name: "Treeant Branch".into(),
        qty: 1,
        max_stack: 10,
        kind: "Item".into(),
        slot: "None".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
    }]);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: can complete False, counting the bank True".to_string()), "{logged:#?}");
}

const RELOGIN: &str = r#"
using Skua.Core.Interfaces;

public class Relog
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Log($"relogged {bot.Servers.EnsureRelogin("fak")}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn relogging_by_name_matches_part_of_a_server_name() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-relogin");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Relog.cs");
    std::fs::write(&script, RELOGIN).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let logins = fake.logins.load(std::sync::atomic::Ordering::SeqCst);

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: relogged True".to_string()), "{logged:#?}");
    assert_eq!(logins, 2, "{logged:#?}");
}

const AURA_TIME: &str = r#"
using Skua.Core.Interfaces;

public class AuraTime
{
    public void ScriptMain(IScriptInterface bot)
    {
        var seal = bot.Self.GetAura("Righteous Seal");
        bot.Log($"seal x{seal?.Value} with {(seal?.RemainingTime is > 60 and <= 120 ? "minutes" : $"{seal?.RemainingTime} s")} left");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn an_aura_reports_the_time_it_has_left() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-aura-time");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("AuraTime.cs");
    std::fs::write(&script, AURA_TIME).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    fake.session.world.me.as_mut().unwrap().auras.push(Aura {
        name: "Righteous Seal".into(),
        stacks: 2,
        until: Some(std::time::Instant::now() + std::time::Duration::from_secs(120)),
        persist: false,
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"info: seal x2 with minutes left".to_string()), "{logged:#?}");
}

const JUMP_WAIT: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class JumpAway
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Bot.Wait.ForTrue(() => Bot.Map.Cells.Contains("Blank"), 50);
        Bot.Map.Jump("r2", "Left");
        Core.JumpWait();
        Core.Logger($"cells {string.Join(",", Bot.Map.Cells)}, waited in {Bot.Player.Cell}");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn jump_wait_leaves_the_monster_cell_for_one_the_map_file_names() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-jump-wait");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("JumpAway.cs");
    std::fs::write(&script, JUMP_WAIT).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.ends_with("cells Enter,r1,r2,Side1,Blank, waited in Side1")), "{logged:#?}");
}

const EVERYDAY: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;
using Skua.Core.Options;

public class Everyday
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.EquipClass(ClassType.Farm);
        Core.Logger($"class {Bot.Player.CurrentClass?.Name}, skills running {Bot.Skills.TimerRunning}");
        Core.Logger($"has dust {Core.CheckInventory("Bone Dust", 3)}, in inventory {Bot.Inventory.Contains("Bone Dust", 3)}");
        Core.ToBank("Bone Dust");
        Core.Logger($"after banking, in bank {Bot.Bank.Contains("Bone Dust")}");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_equips_its_farm_class_and_moves_items_through_the_bank() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-everyday");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("Skua/options")).unwrap();
    std::fs::write(home.join("Skua/options/CBO_Storage(hero).txt"), "FarmClassSelect: Rogue\nFarmModeSelect: Base\n").unwrap();
    let script = dir.join("Everyday.cs");
    std::fs::write(&script, EVERYDAY).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%equipItem%5%15652%") {
                wire.object(r#"{"cmd":"equipItem","uid":1001,"ItemID":15652,"strES":"ar"}"#);
            } else if msg.contains("%bankToInv%5%5%15%") {
                wire.object(r#"{"cmd":"bankToInv","ItemID":"5"}"#);
            } else if msg.contains("%bankFromInv%5%5%15%") {
                wire.object(r#"{"cmd":"bankFromInv","ItemID":5,"bSuccess":1}"#);
            }
        },
        sink,
    );
    let class = Item {
        id: ItemId(15652),
        char_item: Some(CharItemId(78)),
        name: "Rogue".into(),
        qty: 21980,
        max_stack: 1,
        kind: "Class".into(),
        slot: "ar".into(),
        equipped: false,
        ac: false,
        member: false,
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
    };
    let dust = Item { id: ItemId(5), char_item: Some(CharItemId(15)), name: "Bone Dust".into(), qty: 3, max_stack: 50, kind: "Resource".into(), slot: "None".into(), ..class.clone() };
    fake.session.world.me.as_mut().unwrap().bank_slots = 10;
    fake.session.world.inventory.push(class);
    fake.session.world.set_bank(vec![dust]);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.ends_with("class Rogue, skills running True")), "{logged:#?}");
    assert!(logged.iter().any(|l| l.ends_with("has dust True, in inventory True")), "{logged:#?}");
    assert!(logged.iter().any(|l| l.ends_with("after banking, in bank True")), "{logged:#?}");
}

const JOIN: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class JoinAnother
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.Join("yulgar", "Room", "Center");
        Core.Logger($"now in {Bot.Map.FullName} at {Bot.Player.Cell}");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_joins_another_map_in_a_private_room() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-join");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("JoinAnother.cs");
    std::fs::write(&script, JOIN).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if let Some(room) = msg.strip_prefix("%xt%zm%cmd%1%tfer%hero%yulgar-").and_then(|r| r.split('%').next()) {
                wire.object(&format!(
                    r#"{{"cmd":"moveToArea","areaId":7,"areaName":"yulgar-{room}","strMapName":"yulgar","mondef":[],"monmap":[],"monBranch":[],
                    "uoBranch":[{{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}}]}}"#
                ));
                wire.send(&format!("%xt%server%-1%You joined \"yulgar-{room}\"%"));
            }
        },
        sink,
    );
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.contains("now in yulgar-") && l.ends_with("at Room")), "{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%moveToCell%7%Room%Center%")), "{sent:#?}");
}

const KILL_QUEST: &str = r#"
//cs_include Scripts/CoreBots.cs
//cs_include Scripts/CoreStory.cs
using Skua.Core.Interfaces;

public class StoryKill
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;
    public CoreStory Story = new();

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Story.KillQuest(11, "farm", "Treeant");
        Core.Logger($"holding {Bot.Inventory.GetQuantity("Twilly Twig")} Twilly Twig");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_story_kills_for_a_quest_and_turns_it_in() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-story-kill");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("StoryKill.cs");
    std::fs::write(&script, KILL_QUEST).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(&QUEST_11.replace(r#""iSlot":-1,"iValue":0"#, r#""iSlot":3,"iValue":1"#));
            } else if msg.starts_with("%xt%zm%tryQuestComplete%5%11%") {
                farm_quest_server(msg, wire);
                wire.object(r#"{"cmd":"updateQuest","iIndex":3,"iValue":1}"#);
            } else {
                farm_quest_server(msg, wire);
            }
        },
        sink,
    );
    fake.session.world.me.as_mut().unwrap().quest_slots = vec!["0".repeat(100)];
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%11%")), "{sent:#?}");
    assert!(logged.iter().any(|l| l.ends_with("holding 1 Twilly Twig")), "{logged:#?}");
}

const MAP_ITEM_QUEST: &str = r#"
//cs_include Scripts/CoreBots.cs
//cs_include Scripts/CoreStory.cs
using Skua.Core.Interfaces;

public class StoryMapItem
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;
    public CoreStory Story = new();

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Story.MapItemQuest(12, "farm", 55);
        Core.Logger($"quest 12 done {Bot.Quests.IsUnlocked(13)}");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_story_picks_up_a_map_item_for_a_quest() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-story-map-item");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("StoryMapItem.cs");
    std::fs::write(&script, MAP_ITEM_QUEST).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(
        FARM,
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{
                    "12":{"QuestID":12,"sName":"Find the Note","bOnce":true,"iSlot":3,"iValue":1,"bUpg":false,"iLvl":0,"turnin":[{"ItemID":900,"iQty":1}],
                    "oItems":{"900":{"ItemID":900,"sName":"Note Shard","iStk":1,"bTemp":1}},"oRewards":{},"reward":[]},
                    "13":{"QuestID":13,"sName":"Next","bOnce":true,"iSlot":3,"iValue":2,"bUpg":false,"iLvl":0,"turnin":[],"oItems":{},"oRewards":{},"reward":[]}}}"#,
                );
            } else if msg.starts_with("%xt%zm%acceptQuest%5%12%") {
                wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":12,"msg":"success"}"#);
            } else if msg.starts_with("%xt%zm%getMapItem%5%55%") {
                wire.object(r#"{"cmd":"addItems","items":{"900":{"ItemID":900,"sName":"Note Shard","iQty":1,"iStk":1,"bTemp":1}}}"#);
            } else if msg.starts_with("%xt%zm%tryQuestComplete%5%12%") {
                wire.object(r#"{"cmd":"turnIn","sItems":"900:1"}"#);
                wire.object(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":12,"sName":"Find the Note","rewardObj":{"intGold":0}}"#);
                wire.object(r#"{"cmd":"updateQuest","iIndex":3,"iValue":1}"#);
            }
        },
        sink,
    );
    fake.session.world.me.as_mut().unwrap().quest_slots = vec!["0".repeat(100)];
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%getMapItem%5%55%")), "{sent:#?}");
    assert!(logged.iter().any(|l| l.ends_with("quest 12 done True")), "{logged:#?}");
}

const REGISTERED: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class Registered
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.RegisterQuests(11);
        Core.HuntMonster("farm", "Treeant", "Treeant Branch", 1, isTemp: false);
        Bot.Wait.ForTrue(() => Bot.Inventory.Contains("Twilly Twig"), 100);
        Core.CancelRegisteredQuests();
        Core.Logger($"holding {Bot.Inventory.GetQuantity("Twilly Twig")} Twilly Twig");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_turns_in_registered_quests_while_it_hunts() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-registered");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("Registered.cs");
    std::fs::write(&script, REGISTERED).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, farm_quest_server, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%11%")), "{sent:#?}");
    assert!(logged.iter().any(|l| l.ends_with("holding 1 Twilly Twig")), "{logged:#?}");
}

const KILL_MONSTER: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class KillInCell
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.EnsureAccept(11);
        Core.KillMonster("farm", "r2", "Left", "Treeant", "Treeant Branch", 1, isTemp: false);
        Core.Logger($"branches {Bot.Inventory.GetQuantity("Treeant Branch")} from {Bot.Player.Cell}");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_kills_a_monster_in_a_named_cell_for_an_item() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-kill-monster");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("KillInCell.cs");
    std::fs::write(&script, KILL_MONSTER).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, farm_quest_server, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let logged = logged.lock().unwrap().clone();
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}\n{sent:#?}");
    }
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(sent.iter().any(|m| m.starts_with("%xt%zm%moveToCell%5%r2%Left%")), "{sent:#?}");
    assert!(logged.iter().any(|l| l.contains("branches 1 from")), "{logged:#?}");
}

const HOUSE: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class HouseItems
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.Logger($"owns balloons {Core.CheckInventory("Blue Balloons")}, house {Bot.House.UsedSlots}/{Bot.House.Slots}, in the bag {Bot.Inventory.Contains("Blue Balloons")}");
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_finds_an_item_kept_in_the_house() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-house");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("HouseItems.cs");
    std::fs::write(&script, HOUSE).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    fake.session.world.me.as_mut().unwrap().house_slots = 95;
    fake.session.world.house.push(Item {
        id: ItemId(70),
        char_item: Some(CharItemId(22)),
        name: "Blue Balloons".into(),
        qty: 1,
        max_stack: 1,
        kind: "Floor Item".into(),
        slot: "hi".into(),
        equipped: false,
        ac: false,
        member: false,
        house: true,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
    });
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.ends_with("owns balloons True, house 1/95, in the bag False")), "{logged:#?}");
}
