#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use aqx_core::{Event, Item, Outcome, ScriptError, Sink};
use aqx_protocol::ItemId;
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
        link: String::new(),
        meta: String::new(),
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
