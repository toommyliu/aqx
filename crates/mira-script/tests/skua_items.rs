mod support;

use mira_client::{Handle, Server};
use mira_script::{Account, Cancel, Outcome};
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::{FARM, Fake};

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
        bot.Inventory.EquipItem(500);
        bot.Inventory.EquipItem(404);
        bot.Log($"cape equipped {bot.Inventory.IsEquipped(500)}");

        var skill = bot.Flash.GetArrayObject<dynamic>("world.actions.active", 5);
        bot.Log($"item skill {skill.slot} {skill.nam}");
        bot.Flash.CallGameFunction("world.testAction", JsonConvert.DeserializeObject<ExpandoObject>(JsonConvert.SerializeObject(skill)));

        bot.Map.Jump("r2", "Left");
        for (int i = 0; i < 50 && !afk; i++)
            bot.Sleep(20);
        bot.Log($"afk {afk}");
    }
}
"#;

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
        Core.FarmClass = "Rogue";
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

fn shop_server(message: &str, wire: &mut support::Wire) {
    if message.contains("%loadShop%5%1348%") {
        wire.object(r#"{"cmd":"loadShop","shopinfo":{"ShopID":1348,"sName":"Featured Gear Shop","Location":"farm","items":[{"ItemID":102658,"ShopItemID":68071,"sName":"Sword","iCost":300,"iStk":1,"iQty":1,"sType":"Sword","sES":"Weapon"},{"ItemID":5,"ShopItemID":6,"sName":"Hat","iCost":100,"iStk":1,"iQty":1,"sType":"Helm","sES":"he"}]}}"#);
    } else if message.contains("%buyItem%5%102658%1348%68071%1%") {
        wire.object(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":102658,"CharItemID":77,"iQty":1,"bBank":0}"#);
    } else if message.contains("%buyItem%5%5%1348%6%1%") {
        wire.object(
            r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":5,"CharItemID":78,"iQty":1,"bBank":0}"#,
        );
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_and_core_advanced_buy_from_a_shop() {
    let (outcome, logs, sent) = Fake::start(shop_server).run(BUY);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert_eq!(
        sent.iter()
            .filter(|m| m.contains("%buyItem%5%102658%1348%68071%1%"))
            .count(),
        1,
        "{sent:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("holding 1 Sword and 1 Hat")),
        "{logs:?}"
    );
    assert!(
        logs.iter()
            .any(|l| l.contains("shops loaded: 1348 with 2 items")),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_dropped_connection_stops_core_bots_without_crashing_the_host() {
    let (outcome, logs, sent) = Fake::start(|message, wire| {
        if message.contains("%gar%") {
            wire.hang_up();
        } else if message.contains("%getQuests%") {
            wire.object(QUEST_11);
        } else if message.contains("%acceptQuest%") {
            wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#);
        }
    })
    .run(HUNT);
    assert!(
        sent.iter().any(|packet| packet.contains("%gar%")),
        "{sent:?}"
    );
    assert!(
        matches!(outcome, Outcome::Stopped(_)),
        "{outcome:?} {logs:?}"
    );
    assert!(
        !logs
            .iter()
            .any(|l| l.contains("Unhandled exception") || l.contains("still running")),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_script_reads_raw_packets_and_zone_changes_the_way_skua_hands_them_over() {
    let (outcome, logs, _) = Fake::start(|m, w| {
        if m.contains("%moveToCell%") {
            w.object(r#"{"cmd":"ct","anims":[{"msg":"Treeant prepares a counter attack!"}]}"#);
            w.send("%xt%uotls%-1%player1%afk:true%");
            w.object(r#"{"cmd":"event","args":{"zoneSet":"B"}}"#);
        }
    })
    .run(PACKETS);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "json: ct Treeant prepares a counter attack!",
        "str: uotls -1 player1 afk:true",
        "zone: B",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn logging_out_without_auto_relogin_keeps_the_script_running() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(LOGOUT);
    assert!(outcome == Outcome::Finished, "{outcome:?} {logs:?}");
    assert!(logs.iter().any(|l| l.contains("still running")), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_reads_the_account_equips_and_drinks_a_potion_and_hears_afk() {
    let items = json!([
        {"ItemID":11857,"CharItemID":11,"sName":"Health Potion","iQty":3,"iStk":10,"sType":"Item","sES":"None","sMeta":"457","sLink":"potion","sFile":"items/potion.swf"},
        {"ItemID":500,"CharItemID":12,"sName":"Bare Cape","iQty":1,"iStk":1,"sType":"Cape","sES":"ba"}
    ]);
    let data = json!({"CharID":42,"intAQ":1,"intActivationFlag":5});
    let (outcome, logs, sent) = Fake::start_config(FARM, items, json!([]), data, |m, w| {
        if m.contains("%geia%") {
            w.object(r#"{"cmd":"seia","o":{"id":457,"cd":60000,"tgt":"f"},"iRes":1}"#);
        } else if m.contains("%equipItem%") {
            w.object(r#"{"cmd":"equipItem","uid":1001,"ItemID":500,"strES":"ba"}"#);
        } else if m.contains("%gar%") {
            let act = m.split('%').nth(5).unwrap();
            w.object(&format!(
                r#"{{"cmd":"ct","sarsa":[{{"actID":{act},"cInf":"p:1001","iRes":1,"a":[]}}]}}"#
            ));
        } else if m.contains("%moveToCell%5%r2%") {
            w.send("%xt%uotls%-1%player1%afk:true%");
        }
    })
    .run(ACCOUNT_AND_POTION);
    assert_eq!(outcome, Outcome::Finished, "{logs:?} {sent:?}");
    for expected in [
        "char 42, aq 1, verified True",
        "potion None 457 potion items/potion.swf",
        "equipped True",
        "cape equipped True",
        "item skill 5 Item",
        "afk True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert!(
        sent.iter()
            .any(|m| m.contains("%geia%") && m.contains("%potion%457%11857%")),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .any(|m| m.contains("%gar%") && m.contains("i1>")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_sees_a_full_misc_pool_through_inv_cat() {
    let items = json!([{"ItemID":5,"CharItemID":15,"sName":"Bone Dust","iQty":3,"iStk":50,"sType":"Resource","sES":"None"}]);
    let (outcome, logs, _) =
        Fake::start_config(FARM, items, json!([]), json!({"iBagSlots":1}), |_, _| {})
            .run(MISC_POOL);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        logs.contains(
            &"misc 1/1, room for more dust True, a new resource False, a sword True".into()
        ),
        "{logs:?}"
    );
}

struct RelogAccount(Mutex<Vec<mira_client::testing::Harness>>);
impl Account for RelogAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        Ok(vec![Server {
            name: "Example".into(),
            host: "localhost".into(),
            port: 5588,
            players: 1,
            capacity: 1000,
            online: true,
            members_only: false,
        }])
    }
    fn login(&self, server: &str) -> Result<Handle, String> {
        assert_eq!(server, "Example");
        let h = mira_client::testing::Harness::start(Default::default(), |listener| {
            let mut peer = mira_client::testing::Peer::accept(&listener);
            peer.serve_to_ready();
            peer.expect_logout();
        });
        let handle = h.session.clone();
        self.0.lock().unwrap().push(h);
        Ok(handle)
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn relogging_by_name_matches_part_of_a_server_name() {
    let account = Arc::new(RelogAccount(Mutex::new(vec![])));
    let source = r#"
using Skua.Core.Interfaces;
public class Relog {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"still connected {bot.Servers.EnsureRelogin("amp")}");
        bot.Log($"async connected {bot.Servers.EnsureRelogin(CancellationToken.None).GetAwaiter().GetResult()} {bot.Servers.EnsureRelogin("amp", CancellationToken.None).GetAwaiter().GetResult()}");
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        bot.Log($"cancelled connected {bot.Servers.EnsureRelogin("amp", cancelled.Token).GetAwaiter().GetResult()}");
        bot.Log($"has last host {bot.Servers.LastIP.Length > 0 && bot.Servers.LastIP == bot.Player.ServerIP}");
        bot.Send.Packet("player-death");
        bot.Wait.ForTrue(() => bot.Player.State == 0, 20);
        bot.Log($"cancelled offline {bot.Servers.EnsureRelogin("amp", cancelled.Token).GetAwaiter().GetResult()}");
        bot.Log($"recovered {bot.Servers.EnsureRelogin("amp")}");
        bot.Log($"relogged {bot.Servers.Relogin("amp")}");
    }
}
"#;
    let (outcome, logs, _) = Fake::start(|m, w| {
        if m == "player-death" {
            w.object(r#"{"cmd":"uotls","unm":"player1","o":{"intState":0,"intHP":0}}"#);
        }
    })
    .run_account(
        source,
        &Cancel::default(),
        Default::default(),
        account.clone(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"relogged True".into()), "{logs:?}");
    assert!(logs.contains(&"still connected True".into()), "{logs:?}");
    assert!(
        logs.contains(&"async connected True True".into()),
        "{logs:?}"
    );
    assert!(
        logs.contains(&"cancelled connected True".into()),
        "{logs:?}"
    );
    assert!(logs.contains(&"has last host True".into()), "{logs:?}");
    let mut sessions = account.0.lock().unwrap();
    assert!(logs.contains(&"cancelled offline False".into()), "{logs:?}");
    assert!(logs.contains(&"recovered True".into()), "{logs:?}");
    assert_eq!(sessions.len(), 2);
    while let Some(session) = sessions.pop() {
        session.finish();
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_equips_its_farm_class_and_moves_items_through_the_bank() {
    let items = json!([{"ItemID":15652,"CharItemID":78,"sName":"Rogue","iQty":21980,"iStk":1,"sType":"Class","sES":"ar","EnhID":1,"EnhLvl":1}]);
    let bank = json!([{"ItemID":5,"CharItemID":15,"sName":"Bone Dust","iQty":3,"iStk":50,"sType":"Resource","sES":"None"}]);
    let (outcome, logs, sent) =
        Fake::start_config(FARM, items, bank, json!({"iBankSlots":10}), |m, w| {
            if m.contains("%equipItem%5%15652%") {
                w.object(r#"{"cmd":"equipItem","uid":1001,"ItemID":15652,"strES":"ar"}"#);
            } else if m.contains("%bankToInv%5%5%15%") {
                w.object(r#"{"cmd":"bankToInv","ItemID":"5"}"#);
            } else if m.contains("%bankFromInv%5%5%15%") {
                w.object(r#"{"cmd":"bankFromInv","ItemID":5,"bSuccess":1}"#);
            }
        })
        .run(EVERYDAY);
    assert_eq!(outcome, Outcome::Finished, "{logs:?} {sent:?}");
    for expected in [
        "class Rogue, skills running True",
        "has dust True, in inventory True",
        "after banking, in bank True",
    ] {
        assert!(logs.iter().any(|l| l.ends_with(expected)), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_joins_another_map_in_a_private_room() {
    let (outcome, logs, sent) = Fake::start(|m,w| {
        if let Some(room) = m.strip_prefix("%xt%zm%cmd%5%tfer%player1%yulgar-").and_then(|r| r.split('%').next()) {
            w.object(&format!(r#"{{"cmd":"moveToArea","areaId":7,"areaName":"yulgar-{room}","strMapName":"yulgar","mondef":[],"monmap":[],"monBranch":[],"uoBranch":[{{"entID":1001,"strUsername":"player1","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}}]}}"#));
            w.send("<msg t='sys'><body action='joinOK' r='0'><uLs r='7'></uLs></body></msg>");
            w.send(&format!("%xt%server%-1%You joined \"yulgar-{room}\"!%"));
        }
    }).run(JOIN);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        logs.iter()
            .any(|l| l.contains("now in yulgar-") && l.ends_with("at Room")),
        "{logs:?}"
    );
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%moveToCell%7%Room%Center%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_finds_an_item_kept_in_the_house() {
    let items = json!([{"ItemID":70,"CharItemID":22,"sName":"Blue Balloons","iQty":1,"iStk":1,"sType":"Floor Item","sES":"hi","bHouse":true}]);
    let (outcome, logs, _) =
        Fake::start_config(FARM, items, json!([]), json!({"iHouseSlots":95}), |_, _| {}).run(HOUSE);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        logs.iter()
            .any(|l| l.ends_with("owns balloons True, house 1/95, in the bag False")),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn sent_packets_raise_packet_cell_quest_and_buy_events_before_any_reply() {
    let source = r#"
using Skua.Core.Interfaces;
public class SentEvents {
    public void ScriptMain(IScriptInterface bot) {
        int packets = 0;
        ((dynamic)bot.Events).PacketReceived += (Skua.Core.Interfaces.PacketEventHandler)(text => { bot.Log($"sent {text}"); Interlocked.Increment(ref packets); });
        bot.Events.CellChanged += (map,cell,pad) => bot.Log($"cell {map} {cell} {pad}");
        bot.Events.QuestAccepted += id => bot.Log($"quest {id}");
        bot.Events.TryBuyItem += (shop,item,listing) => bot.Log($"buy {shop} {item} {listing}");
        bot.Send.Packet("%xt%zm%moveToCell%5%r2%Left%");
        bot.Send.Packet("%xt%zm%acceptQuest%5%11%");
        bot.Send.Packet("%xt%zm%buyItem%5%5%1348%6%1%");
        for (int i = 0; i < 100 && Volatile.Read(ref packets) < 3; i++) bot.Sleep(20);
        bot.Log($"packets {packets}");
    }
}
"#;
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "cell farm r2 Left",
        "quest 11",
        "buy 1348 5 6",
        "packets 3",
        "sent %xt%zm%acceptQuest%5%11%",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn send_packet_passes_json_and_xml_through_without_validation() {
    let source = r#"
using Skua.Core.Interfaces;
public class PacketTypes {
    public void ScriptMain(IScriptInterface bot) {
        bot.Send.Packet("{\"example\":true}", "Json");
        bot.Send.Packet("<example/>", "Xml");
        bot.Send.Packet("unparsed text", "String");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for packet in ["{\"example\":true}", "<example/>", "unparsed text"] {
        assert!(sent.contains(&packet.into()), "{sent:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn bank_swap_overloads_exchange_items_atomically_in_full_bags() {
    let source = r#"
using Skua.Core.Interfaces;
public class SwapItems {
    public void ScriptMain(IScriptInterface bot) {
        bot.Bank.Load();
        bot.Log($"swap {bot.Bank.Swap("Blade", "Hat")}");
        bot.Log($"swap {bot.Bank.Swap(6, 5)}");
        bot.Log($"swap {bot.Bank.Swap(bot.Inventory.Items.Single(), bot.Bank.Items.Single())}");
        bot.Log($"swap {bot.Bank.EnsureSwap("Hat", "Blade", false)}");
        bot.Log($"swap {bot.Bank.EnsureSwap(5, 6)}");
        bot.Log($"missing {bot.Bank.Swap(999, 5)} {bot.Bank.EnsureSwap("missing", "Blade")}");
        bot.Log($"holding {bot.Inventory.Items.Single().Name}, banked {bot.Bank.Items.Single().Name}");
    }
}
"#;
    let items =
        json!([{"ItemID":5,"CharItemID":15,"sName":"Blade","iQty":1,"iStk":1,"sType":"Sword"}]);
    let bank = json!([{"ItemID":6,"CharItemID":16,"sName":"Hat","iQty":1,"iStk":1,"sType":"Helm"}]);
    let (outcome, logs, sent) = Fake::start_config(
        FARM,
        items,
        bank,
        json!({"iBagSlots":1,"iBankSlots":1,"iBankCount":1}),
        |m, w| {
            if m.contains("%bankSwapInv%") {
                let parts: Vec<_> = m.split('%').collect();
                w.object(&format!(
                    r#"{{"cmd":"bankSwapInv","invItemID":{},"bankItemID":{}}}"#,
                    parts[5], parts[7]
                ));
            }
        },
    )
    .run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert_eq!(
        logs.iter().filter(|l| *l == "swap True").count(),
        5,
        "{logs:?}"
    );
    assert!(logs.contains(&"missing False False".into()), "{logs:?}");
    assert!(
        logs.contains(&"holding Hat, banked Blade".into()),
        "{logs:?}"
    );
    assert_eq!(
        sent.iter().filter(|m| m.contains("%bankSwapInv%")).count(),
        5,
        "{sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|m| m.contains("%bankFromInv%") || m.contains("%bankToInv%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn house_deposits_and_shop_sales_use_the_selected_item_and_quantity() {
    let source = r#"
using Skua.Core.Interfaces;
public class MoveItems {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"house {bot.House.ToBank(bot.House.Items.First(i => i.ID == 70))} {bot.House.EnsureToBank("Lamp")} {bot.House.EnsureToBank(72)} {bot.House.EnsureToBank(999)}");
        bot.Shops.SellItem("Dust", 2);
        bot.Shops.SellItem(5);
        bot.Shops.SellItem(6, 99);
        bot.Shops.SellItem("missing");
        bot.Shops.SellItem(7);
        bot.Log($"remaining {bot.House.Items.Count} {bot.Inventory.Items.Count}, banked {bot.Bank.Items.Count}");
    }
}
"#;
    let items = json!([
        {"ItemID":70,"CharItemID":170,"sName":"Chair","iQty":1,"iStk":1,"sType":"Floor Item","bHouse":true},
        {"ItemID":71,"CharItemID":171,"sName":"Lamp","iQty":1,"iStk":1,"sType":"Floor Item","bHouse":true},
        {"ItemID":72,"CharItemID":172,"sName":"Rug","iQty":1,"iStk":1,"sType":"Floor Item","bHouse":true},
        {"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":5,"iStk":50,"sType":"Resource"},
        {"ItemID":6,"CharItemID":16,"sName":"Stone","iQty":3,"iStk":50,"sType":"Resource"},
        {"ItemID":7,"CharItemID":17,"sName":"Equipped Blade","iQty":1,"iStk":1,"sType":"Sword","sES":"Weapon","bEquip":true}
    ]);
    let (outcome, logs, sent) =
        Fake::start_config(FARM, items, json!([]), json!({"iBankSlots":10}), |m, w| {
            let p: Vec<_> = m.split('%').collect();
            if m.contains("%bankFromInv%") {
                w.object(&format!(
                    r#"{{"cmd":"bankFromInv","ItemID":{},"bSuccess":1}}"#,
                    p[5]
                ));
            } else if m.contains("%sellItem%") {
                w.object(&format!(
                    r#"{{"cmd":"sellItem","CharItemID":{},"iQty":{},"bSuccess":1}}"#,
                    p[7], p[6]
                ));
            }
        })
        .run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?} {sent:?}");
    assert!(
        logs.contains(&"house True True True False".into()),
        "{logs:?}"
    );
    assert!(logs.contains(&"remaining 0 1, banked 3".into()), "{logs:?}");
    for packet in [
        "%xt%zm%sellItem%5%5%2%15%",
        "%xt%zm%sellItem%5%5%3%15%",
        "%xt%zm%sellItem%5%6%3%16%",
    ] {
        assert!(sent.contains(&packet.into()), "{sent:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn faction_list_exposes_total_and_rank_progress() {
    let source = r#"
using Skua.Core.Interfaces;
public class Reputation {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"initial {bot.Reputation.FactionList.Count}");
        bot.Send.Packet("faction-update");
        bot.Wait.ForTrue(() => bot.Reputation.FactionList.Count == 1, 20);
        var faction = bot.Reputation.FactionList.Single();
        bot.Log($"faction {faction.ID} {faction.Name} {faction.Rank} {faction.TotalRep} {faction.Rep} {faction.RequiredRep} {faction.RemainingRep}");
    }
}
"#;
    let (outcome, logs, _) = Fake::start(|m, w| {
        if m == "faction-update" {
            w.object(
                r#"{"cmd":"addFaction","faction":{"FactionID":4,"sName":"Good","iRep":1000}}"#,
            );
        }
    })
    .run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"initial 0".into()), "{logs:?}");
    assert!(
        logs.contains(&"faction 4 Good 2 1000 100 2700 2600".into()),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn flash_actions_return_false_when_a_shop_request_is_refused() {
    let source = r#"
using Skua.Core.Interfaces;
public class RefusedShop {
    public void ScriptMain(IScriptInterface bot) {
        bot.Shops.Load(7);
        bot.Shops.Load(7);
        bot.Log($"loaded {bot.Shops.IsLoaded}");
        bot.Log($"refused {bot.Flash.CallGameFunction("world.sendLoadShopRequest", 7)}");
        bot.Log("continued");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start(|m,w| {
        if m.contains("%loadShop%") {
            w.object(r#"{"cmd":"loadShop","shopinfo":{"ShopID":7,"sName":"Limited","Location":"othermap","bLimited":true,"items":[]}}"#);
        }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in ["loaded True", "refused false", "continued"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert_eq!(
        sent.iter().filter(|p| p.contains("%loadShop%")).count(),
        1,
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn ensure_bank_transfers_retry_refusals_but_plain_transfers_try_once() {
    let source = r#"
using Skua.Core.Interfaces;
public class Transfers {
    public void ScriptMain(IScriptInterface bot) {
        bot.Options.MaximumTries = 3;
        bot.Options.ActionDelay = 80;
        bot.Bank.Load();
        bot.Log($"plain {bot.Inventory.ToBank(5)}");
        bot.Log($"ensure {bot.Inventory.EnsureToBank(5)}");
        bot.Log($"missing {bot.Inventory.EnsureToBank(99)}");
        bot.Log($"withdraw {bot.Bank.EnsureToInventory(5, false)}");
        bot.Log($"exhausted {bot.Inventory.EnsureToBank(5)}");
    }
}
"#;
    let items = json!([{"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":3,"iStk":50,"sType":"Resource","sES":"None"}]);
    let mut attempts = 0;
    let (outcome, logs, sent) = Fake::start_config(
        FARM,
        items,
        json!([]),
        json!({"iBankSlots":10}),
        move |m, w| {
            if m.contains("%bankFromInv%") {
                attempts += 1;
                w.object(if attempts == 3 {
                    r#"{"cmd":"bankFromInv","ItemID":5,"bSuccess":1}"#
                } else {
                    r#"{"cmd":"bankFromInv","ItemID":5,"bSuccess":0,"msg":"Not now"}"#
                });
            } else if m.contains("%bankToInv%") {
                w.object(r#"{"cmd":"bankToInv","ItemID":"5"}"#);
            }
        },
    )
    .run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "plain False",
        "ensure True",
        "missing False",
        "withdraw True",
        "exhausted False",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert_eq!(
        sent.iter().filter(|m| m.contains("%bankFromInv%")).count(),
        6,
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn drop_rejection_preserves_script_lists_and_follows_option_precedence() {
    let source = r#"
using Skua.Core.Interfaces;
public class Rejection {
    public void ScriptMain(IScriptInterface bot) {
        bot.Drops.Interval = 20;
        int changes = 0;
        bot.Drops.PropertyChanged += (_, e) => Interlocked.Increment(ref changes);
        bot.Drops.Add("Dust", "dust");
        bot.Drops.Remove("DUST");
        bot.Log($"names {string.Join(",", bot.Drops.ToPickup)}");
        bot.Drops.RejectElse = true;
        bot.Send.Packet("offer-drops");
        bot.Wait.ForTrue(() => bot.Drops.CurrentDropInfos.Count() == 3, 20);
        bot.Drops.RejectExcept(5);
        bot.Drops.RejectAll();
        bot.Log($"retained {bot.Drops.CurrentDropInfos.Count()} raw {bot.Flash.Call<bool>("rejectExcept", "dust")}");
        bot.Options.AcceptAllDrops = true;
        bot.Options.RejectAllDrops = true;
        bot.Log($"reject {bot.Options.RejectAllDrops} accept {bot.Options.AcceptAllDrops}");
        bot.Drops.Start();
        bot.Sleep(150);
        bot.Drops.Stop();
        bot.Options.RejectAllDrops = false;
        bot.Options.AcceptACDrops = true;
        bot.Drops.RejectAll();
        bot.Log($"after ac {string.Join(",", bot.Drops.CurrentDrops)} changes {changes > 0}");
        bot.Options.AcceptAllDrops = true;
        bot.Log($"accept {bot.Options.AcceptAllDrops} reject {bot.Options.RejectAllDrops}");
        bot.Options.AcceptAllDrops = false;
    }
}
"#;
    let (outcome, logs, sent) = Fake::start(|m,w| {
        if m == "offer-drops" {
            w.object(r#"{"cmd":"dropItem","items":{"5":{"ItemID":5,"sName":"Dust","iQty":1,"iStk":10},"6":{"ItemID":6,"sName":"Ash","iQty":1,"iStk":10},"7":{"ItemID":7,"sName":"Coin","iQty":1,"iStk":10,"bCoins":1}}}"#);
        } else if m.contains("%getDrop%5%7%") {
            w.object(r#"{"cmd":"getDrop","bSuccess":1,"ItemID":7,"CharItemID":77,"iQty":1,"iQtyNow":1,"bBank":0}"#);
        }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "names Dust,dust",
        "retained 3 raw True",
        "reject True accept False",
        "after ac Dust,Ash changes True",
        "accept True reject False",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert!(
        !sent
            .iter()
            .any(|m| m.contains("%getDrop%5%5%") || m.contains("%getDrop%5%6%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn shop_flash_fields_cache_capacity_and_purchase_requirements_match_skua() {
    let source = r#"
using Skua.Core.Interfaces;
using Skua.Core.Models.Shops;
public class ShopRequirements {
    public void ScriptMain(IScriptInterface bot) {
        bot.Send.Packet("%xt%zm%loadShop%5%1348%");
        bot.Wait.ForTrue(() => bot.Shops.LoadedCache.Count == 1, 20);
        bot.Log($"cache {bot.Shops.LoadedCache.Count}");
        bot.Log($"lock {bot.Flash.GetGameObject<int>("world.shopinfo.iIndex")} {bot.Flash.GetGameObject<string>("world.shopinfo.sField")} {bot.Flash.GetGameObject<int>("world.shopinfo.reqItems")}");
        dynamic raw = bot.Flash.GetGameObject<List<dynamic>>("world.shopinfo.items")!.Single(i => i.ItemID == 9);
        bot.Log($"item {raw.iQSindex} {raw.iQSvalue} {raw.sQuest} {raw.iLvl} {raw.FactionID} {raw.sFaction} {raw.TokName} {raw.iQtyRemain}");
        var item = bot.Shops.Items.Single(i => i.ID == 9);
        bot.Log($"space one {bot.Inventory.HasSpaceFor(item, 1)} two {bot.Inventory.HasSpaceFor(item, 2)} max {bot.Flash.CallGameFunction<int>("world.maximumShopBuys", item)}");
        bot.Log($"unmet {string.Join("|", bot.Shops.GetUnmetPurchaseRequirements(item, 1))}");
        bot.Shops.BuyItem(9, quantity: 0);
        bot.Shops.BuyItem(9, quantity: 1);
        bot.Shops.BuyItem(404);
        bot.Log($"bundles {bot.Flash.CallGameFunction<int>("world.maximumShopBuys", new ShopItem { ID = 10, Cost = 500, Quantity = 3, MaxStack = 10 })} raw {bot.Flash.CallGameFunction<int>("world.maximumShopBuys", new { ItemID = 10, iCost = 500, iQty = 3, iStk = 10 })}");
        object plain = bot.Flash.GetGameObject<List<Newtonsoft.Json.Linq.JObject>>("world.shopinfo.items")!.Single(i => (int)i["ItemID"]! == 10);
        bot.Log($"plain {bot.Flash.CallGameFunction<int>("world.maximumShopBuys", plain)}");
        bot.Log("safe preflight");
    }
}
"#;
    let items =
        json!([{"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":2,"iStk":10,"sType":"Resource"}]);
    let (outcome, logs, sent) = Fake::start_config(FARM, items, json!([]), json!({"iBagSlots":1,"strQuests":"000"}), |m,w| {
        if m.contains("%loadShop%") {
            w.object(r#"{"cmd":"loadShop","shopinfo":{"ShopID":1348,"sName":"Locked Shop","Location":"farm","iIndex":"7","sField":"ia1","reqItems":"12","items":[{"ItemID":9,"ShopItemID":19,"sName":"Ash","iCost":0,"iStk":10,"iQty":1,"sType":"Resource","iQSindex":2,"iQSvalue":3,"sQuest":"A Quest","iLvl":40,"FactionID":4,"sFaction":"Good","TokName":"Dust","iQtyRemain":-1,"turnin":[{"ItemID":5,"sName":"Dust","iQty":1}]},{"ItemID":10,"ShopItemID":20,"sName":"Bundle","iCost":500,"iStk":10,"iQty":3,"sType":"Resource"}]}}"#);
        }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "cache 1",
        "lock 7 ia1 12",
        "item 2 3 A Quest 40 4 Good Dust -1",
        "space one False two True max 2",
        "unmet Quest requirement not met.",
        "bundles 6 raw 2",
        "plain 2",
        "safe preflight",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert!(!sent.iter().any(|m| m.contains("%buyItem%")), "{sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn item_events_deliver_received_quantities_and_sale_details() {
    let source = r#"
using Skua.Core.Interfaces;
public class ItemEvents {
    public void ScriptMain(IScriptInterface bot) {
        int events = 0;
        bot.Bank.Load();
        bot.Events.ItemDropped += (item, added, qty) => { bot.Log($"drop {item.ID} {item.Name} {item.Temp} {added} {qty}"); Interlocked.Increment(ref events); };
        bot.Events.ItemAddedToBank += (item, qty) => { bot.Log($"bank {item.ID} {qty}"); Interlocked.Increment(ref events); };
        bot.Events.ItemBought += id => { bot.Log($"bought {id}"); Interlocked.Increment(ref events); };
        bot.Events.ItemSold += (id, qty, now, cost, ac) => { bot.Log($"sold {id} {qty} {now} {cost} {ac}"); Interlocked.Increment(ref events); };
        bot.Send.Packet("item-events");
        bot.Wait.ForTrue(() => Volatile.Read(ref events) == 5, 20);
        bot.Log($"events {events}");
    }
}
"#;
    let bank = json!([{"ItemID":8,"CharItemID":18,"sName":"Bank Dust","iQty":1,"iStk":10}]);
    let (outcome, logs, _) = Fake::start_with(FARM, json!([]), bank, |m,w| {
        if m == "item-events" {
            w.object(r#"{"cmd":"dropItem","items":{"5":{"ItemID":5,"sName":"Dust","iQty":2,"iStk":10}}}"#);
            w.object(r#"{"cmd":"addItems","items":{"6":{"ItemID":6,"sName":"Temp Dust","iQty":3,"iQtyNow":3,"iStk":10,"bTemp":true}}}"#);
            w.object(r#"{"cmd":"addItems","items":{"8":{"ItemID":8,"CharItemID":18,"iQty":2,"iQtyNow":3,"bBank":1}}}"#);
            w.object(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":9,"CharItemID":19,"iQty":1}"#);
            w.object(r#"{"cmd":"sellItem","CharItemID":20,"iQty":2,"iQtyNow":4,"intAmount":50,"bCoins":1}"#);
        }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "drop 5 Dust False False 0",
        "drop 6 Temp Dust True True 3",
        "bank 8 3",
        "bought 19",
        "sold 20 2 4 50 True",
        "events 5",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn inventory_helpers_and_bank_loaded_setter_match_skua() {
    let source = r#"
using Skua.Core.Interfaces;
public class Helpers {
    public void ScriptMain(IScriptInterface bot) {
        bot.Bank.Load();
        bot.Bank.Loaded = false;
        bot.Log($"loaded {bot.Bank.Loaded}");
        bot.Bank.Loaded = true;
        bot.Log($"helpers {bot.InvHelper.HasAll(new[] {"Dust", "Ash"}, moveToInventory: false)} {bot.InvHelper.HasAny(new[] {5, 99}, moveToInventory: false)} {bot.InvHelper.HasAny(Array.Empty<int>())} {bot.InvHelper.HasAll(new[] {99}, moveToInventory: false)}");
    }
}
"#;
    let items = json!([{"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":2,"iStk":10}]);
    let bank = json!([{"ItemID":6,"CharItemID":16,"sName":"Ash","iQty":2,"iStk":10}]);
    let (outcome, logs, _) = Fake::start_with(FARM, items, bank, |_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in ["loaded False", "helpers True True True False"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_failed_bank_load_leaves_the_bank_unloaded_without_throwing() {
    let source = r#"
using Skua.Core.Interfaces;
public class FailedLoad {
    public void ScriptMain(IScriptInterface bot) {
        bot.Bank.Load();
        bot.Log($"loaded {bot.Bank.Loaded} {bot.Bank.Items.Count}");
    }
}
"#;
    let (outcome, logs, _) =
        Fake::start_with(FARM, json!([]), json!("down"), |_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"loaded False 0".into()), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn shop_selling_protects_local_favorites_unless_disabled() {
    let source = r#"
using Skua.Core.Interfaces;
public class Favorites {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"protect {bot.Shops.ProtectFavorites} favorite {bot.Inventory.IsFavorited(5)}");
        bot.Flash.SetGameObject("litePreference.data.favs", new Dictionary<string, int[]> { ["42"] = new[] {5} });
        bot.Log($"favorite {bot.Inventory.IsFavorited(5)} other {bot.Inventory.IsFavorited(6)}");
        bot.Shops.SellItem(5);
        bot.Log($"protected {bot.Inventory.Contains(5)}");
        bot.Shops.ProtectFavorites = false;
        bot.Shops.SellItem("Dust", 1);
        bot.Log($"remaining {bot.Inventory.GetQuantity(5)}");
    }
}
"#;
    let items =
        json!([{"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":2,"iStk":10,"sType":"Resource"}]);
    let (outcome, logs, sent) = Fake::start_config(FARM, items, json!([]), json!({"CharID":42}), |m,w| {
        if m.contains("%sellItem%") { w.object(r#"{"cmd":"sellItem","CharItemID":15,"iQty":1,"iQtyNow":1,"intAmount":10,"bCoins":0}"#); }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "protect True favorite False",
        "favorite True other False",
        "protected True",
        "remaining 1",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert_eq!(
        sent.iter().filter(|m| m.contains("%sellItem%")).count(),
        1,
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn inventory_items_expose_descriptions_levels_and_purchase_metadata() {
    let source = r#"
using Skua.Core.Interfaces;
public class ItemMetadata {
    public void ScriptMain(IScriptInterface bot) {
        var item = bot.Inventory.GetItem(5)!;
        bot.Log($"item {item.Description} {item.Level} {item.Wearing}");
        bot.Send.Packet("temp-item");
        bot.Wait.ForTrue(() => bot.TempInv.Contains(6), 2);
        var temp = bot.TempInv.GetItem(6)!;
        bot.Log($"temp {temp.Description} {temp.CategoryString} {temp.Meta} {temp.ItemGroup} {temp.Coins}");
        bot.Log($"metadata {bot.Flash.GetGameObject<int>("world.invTree.5.iCost")} {bot.Flash.GetGameObject<int>("world.invTree.5.iHrs")} {bot.Flash.GetGameObject<int>("world.invTree.5.iQSindex")} {bot.Flash.GetGameObject<int>("world.invTree.5.iQSvalue")} {bot.Flash.GetGameObject<string>("world.invTree.5.sQuest")}");
    }
}
"#;
    let items = json!([{"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":2,"sDesc":"A material.","iLvl":40,"iCost":25,"iHrs":12,"iQSindex":3,"iQSvalue":2,"sQuest":"A Quest","bWear":1}]);
    let (outcome, logs, _) = Fake::start_with(FARM, items, json!([]), |m,w| {
        if m == "temp-item" { w.object(r#"{"cmd":"addItems","items":{"6":{"ItemID":6,"sName":"Temp Dust","iQty":1,"iStk":10,"bTemp":1,"sDesc":"A temporary material.","sType":"Quest Item","sMeta":"quest","sES":"None","bCoins":1}}}"#); }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "item A material. 40 True",
        "metadata 25 12 3 2 A Quest",
        "temp A temporary material. Quest Item quest None True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn raw_bank_loads_publish_loaded_state_and_items() {
    let source = r#"
using Skua.Core.Interfaces;
public class RawBank {
    public void ScriptMain(IScriptInterface bot) {
        bot.Send.Packet("%xt%zm%loadBank%5%All%");
        bot.Wait.ForTrue(() => bot.Bank.Loaded, 2);
        bot.Log($"bank {bot.Bank.Loaded} {bot.Bank.GetQuantity(5)}");
        bot.Bank.Loaded = false;
        bot.Bank.Load(false);
        bot.Wait.ForTrue(() => bot.Bank.Loaded, 2);
        bot.Log($"async {bot.Bank.Loaded}");
    }
}
"#;
    let (outcome, logs, _) = Fake::start(|m,w| {
        if m.contains("%loadBank%") { w.object(r#"{"cmd":"loadBank","bitSuccess":1,"items":[{"ItemID":5,"CharItemID":15,"sName":"Dust","iQty":2,"iStk":10}]}"#); }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in ["bank True 2", "async True"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

const LATE_ITEM: &str = r#"
using Skua.Core.Interfaces;

public class LateItem
{
    public void ScriptMain(IScriptInterface bot)
    {
        bool received = false;
        bot.Events.CellChanged += (map, cell, pad) => bot.Sleep(1000);
        bot.Events.ItemDropped += (item, added, quantity) => { if (added && item.Name == "Note Shard") received = true; };
        bot.Map.Jump("r2", "Left");
        bot.Wait.ForTrue(() => received, 40);
        bot.Log($"received {received}, held {bot.TempInv.Contains("Note Shard")}");
    }
}
"#;

#[test]
#[ignore = "run with npm run skua:test"]
fn an_item_event_keeps_its_item_when_the_item_is_gone_before_handlers_run() {
    let (outcome, logs, _) = Fake::start(|message, wire| {
        if message.contains("%moveToCell%") {
            wire.object(r#"{"cmd":"addItems","items":{"900":{"ItemID":900,"sName":"Note Shard","iQty":1,"iStk":1,"bTemp":1}}}"#);
            std::thread::sleep(std::time::Duration::from_millis(300));
            wire.object(r#"{"cmd":"turnIn","sItems":"900:1"}"#);
        }
    })
    .run(LATE_ITEM);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        logs.contains(&"received True, held False".into()),
        "{logs:?}"
    );
}
