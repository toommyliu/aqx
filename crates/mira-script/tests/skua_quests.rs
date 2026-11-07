mod support;

use mira_script::Outcome;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::{FARM, Fake, Wire};

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

const QUEST_REPEATS: &str = r#"
using Skua.Core.Interfaces;

public class QuestRepeats
{
    public void ScriptMain(IScriptInterface bot)
    {
        var quest = bot.Quests.EnsureLoad(12)!;
        bot.Log($"{quest.Name} rewards {string.Join(", ", quest.Rewards.Select(r => r.Name))}");
        bot.Log($"text {quest.Description} / {quest.EndText}");
        var reward = quest.Rewards[0];
        bot.Log($"member {reward.Upgrade} coins {reward.Coins} {reward.Category} {reward.Description}");
    }
}
"#;

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
        Bot.Options.QuestAcceptAndCompleteTries = 1;
        var sword = Bot.Inventory.Items.First(i => i.ID == 7);
        Core.Logger($"sword {sword.CharItemID} pattern {sword.EnhancementPatternID}");
        Core.EnsureAccept(13);
        Core.Logger($"completed {Core.EnsureComplete(13)}");
        Core.Logger($"explicit choice {Core.EnsureComplete(13, 1027)}");
        Core.SetOptions(false);
    }
}
"#;

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

const FULL_CHECK: &str = r#"
using Skua.Core.Interfaces;

public class FullCheck
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Bank.Load();
        bot.Log($"can complete {bot.Quests.CanComplete(11)}, counting the bank {bot.Quests.CanCompleteFullCheck(11)}");
    }
}
"#;

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
        Core.HuntMonster("farm", "Treeant", "Twilly Twig", 1, isTemp: false);
        Bot.Wait.ForTrue(() => Bot.Inventory.Contains("Twilly Twig"), 100);
        Core.CancelRegisteredQuests();
        Core.Logger($"holding {Bot.Inventory.GetQuantity("Twilly Twig")} Twilly Twig");
        Core.SetOptions(false);
    }
}
"#;

const TWO_MAP_ITEMS: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class TwoMapItems
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.GetMapItems(new[] { (55, 1), (56, 1) });
        Core.Logger($"holding {Bot.TempInv.GetQuantity(900)} and {Bot.TempInv.GetQuantity(901)}");
    }
}
"#;

const CLASS_QUEST: &str = r#"
using System.Linq;
using Skua.Core.Interfaces;

public class ClassQuest
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Quests.Load(17, 18);
        var hard = bot.Quests.EnsureLoad(17)!;
        bot.Log($"rank 3 quest available {bot.Quests.IsAvailable(17)}, rank 2 quest available {bot.Quests.IsAvailable(18)}");
        bot.Log($"needs class {hard.RequiredClassID} at {hard.RequiredClassPoints}, pays {hard.Gold} gold {hard.XP} xp, spark is temp {hard.Requirements.First().Temp}");
        bot.Log($"accepted the locked one {bot.Quests.EnsureAccept(17)}");
    }
}
"#;

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
#[ignore = "run with npm run skua:test"]
fn core_bots_hunts_a_monster_for_a_quest_item_and_turns_it_in() {
    let fake = Fake::start(farm_quest_server);
    let (result, logged, sent) = fake.run(HUNT);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    let at = |needle: &str| {
        sent.iter()
            .position(|m| m.contains(needle))
            .unwrap_or_else(|| panic!("never sent {needle}: {sent:#?}\n{logged:#?}"))
    };
    assert!(at("%acceptQuest%5%11%") < at("%moveToCell%5%r2%"));
    assert!(at("%moveToCell%5%r2%") < at(">m:2"));
    assert!(at(">m:2") < at("%getDrop%5%87%"));
    assert!(at("%getDrop%5%87%") < at("%tryQuestComplete%5%11%"));
    assert!(
        logged.iter().any(|l| l.contains("holding 1 Twilly Twig")),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_quest_may_list_the_same_reward_twice() {
    let fake = Fake::start_with(FARM, json!([]), json!([]), |msg, wire| {
        if msg.contains("%getQuests%") {
            wire.object(
                    r#"{"cmd":"getQuests","quests":{"12":{"QuestID":12,"sName":"Twice","sDesc":"Bring supplies","sEndText":"Supplies received","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
                    "turnin":[],"oItems":{},"oRewards":{"itemsS":{"0":{"ItemID":1027,"sName":"Twilly Twig","iStk":1,"bUpg":"1","bCoins":1,"sType":"Staff","sDesc":"A bent twig"},"1":{"ItemID":1027,"sName":"Twilly Twig","iStk":1,"bUpg":"1","bCoins":1,"sType":"Staff","sDesc":"A bent twig"}}},
                    "reward":[{"ItemID":1027,"iQty":1,"iRate":50,"iType":0},{"ItemID":1027,"iQty":2,"iRate":100,"iType":0}]}}}"#,
                );
        }
    });
    let (result, logged, _) = fake.run(QUEST_REPEATS);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged
            .iter()
            .any(|l| l == "Twice rewards Twilly Twig, Twilly Twig"),
        "{logged:#?}"
    );
    assert!(
        logged.contains(&"text Bring supplies / Supplies received".into()),
        "{logged:?}"
    );
    assert!(
        logged.contains(&"member True coins True Staff A bent twig".into()),
        "{logged:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_reads_item_metadata_and_requires_an_explicit_choice_without_a_reward_stack_size() {
    let fake = Fake::start_with(
        FARM,
        json!([{ "ItemID":7,"CharItemID":77,"sName":"Sword","iQty":1,"iStk":1,"sType":"Sword","sES":"Weapon","iLvl":1,"EnhID":3719,"EnhLvl":22,"EnhPatternID":2 }]),
        json!([]),
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{"13":{"QuestID":13,"sName":"Pick One","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
                    "turnin":[],"oItems":{},"oRewards":{"itemsC":{"0":{"ItemID":1027,"sName":"Twilly Twig","iStk":1},"1":{"ItemID":1028,"sName":"Twilly Leaf","iStk":1}}},
                    "reward":[{"ItemID":1027,"iQty":1,"iRate":100,"iType":2},{"ItemID":1028,"iQty":1,"iRate":100,"iType":2}]}}}"#,
                );
            } else if msg.starts_with("%xt%zm%acceptQuest%5%13%") {
                wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":13,"msg":"success"}"#);
            } else if msg.starts_with("%xt%zm%tryQuestComplete%5%13%-1%") {
                wire.object(r#"{"cmd":"ccqr","bSuccess":0,"QuestID":13,"sName":"Pick One","msg":"Missing Turn In Item"}"#);
            } else if msg.starts_with("%xt%zm%tryQuestComplete%5%13%") {
                wire.object(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":13,"sName":"Pick One","rewardObj":{"intGold":0}}"#);
                wire.object(r#"{"cmd":"addItems","items":{"1027":{"ItemID":1027,"sName":"Twilly Twig","iQty":1,"iStk":1,"CharItemID":10}}}"#);
            }
        },
    );
    let (result, logged, sent) = fake.run(CHOICE);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.iter().any(|l| l.ends_with("sword 77 pattern 2")),
        "{logged:#?}"
    );
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%13%-1%false%1%wvz%")),
        "{sent:#?}"
    );
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%13%1027%")),
        "{sent:#?}"
    );
    assert!(
        logged.iter().any(|l| l.ends_with("completed False")),
        "{logged:#?}"
    );
    assert!(
        logged.iter().any(|l| l.ends_with("explicit choice True")),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn update_quest_moves_the_quest_slot_the_way_skuas_fake_packet_does() {
    let fake = Fake::start_config(
        FARM,
        json!([]),
        json!([]),
        json!({"strQuests":"0".repeat(100)}),
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
    );
    let (result, logged, sent) = fake.run(UPDATE_QUEST);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"unlocked False then True, accepted True".to_string()),
        "{logged:#?}"
    );
    assert!(
        !sent.iter().any(|m| m.contains("%updateQuest%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_quest_whose_held_item_is_missing_is_still_available() {
    let fake = Fake::start_with(FARM, json!([]), json!([]), |msg, wire| {
        if msg.contains("%getQuests%") {
            wire.object(
                    r#"{"cmd":"getQuests","quests":{"16":{"QuestID":16,"sName":"Badge Holders","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
                    "turnin":[],"oItems":{},"oRewards":{},"reward":[],"reqd":[{"ItemID":900,"iQty":1}],"oReqd":{"900":{"ItemID":900,"sName":"Badge","iStk":1}}}}}"#,
                );
        }
    });
    let (result, logged, _) = fake.run(AVAILABLE);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"available True".to_string()),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn loading_quests_returns_without_waiting_for_ones_the_server_never_sends() {
    let fake = Fake::start(farm_quest_server);
    let (result, logged, sent) = fake.run(LOAD);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(logged.contains(&"load returned".to_string()), "{logged:#?}");
    assert!(
        logged.contains(&"tree has 11: True".to_string()),
        "{logged:#?}"
    );
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%getQuests%5%11%404%")),
        "{sent:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn the_full_completion_check_counts_the_bank() {
    let fake = Fake::start_with(
        FARM,
        json!([]),
        json!([{"ItemID":87,"CharItemID":9,"sName":"Treeant Branch","iQty":1,"iStk":10,"sType":"Item","sES":"None","bBank":true}]),
        farm_quest_server,
    );
    let (result, logged, _) = fake.run(FULL_CHECK);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"can complete False, counting the bank True".to_string()),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn an_equipped_turn_in_item_still_completes_the_quest() {
    let fake = Fake::start_with(
        FARM,
        json!([{"ItemID":87,"CharItemID":9,"sName":"Treeant Branch","iQty":1,"iStk":10,"sType":"Item","sES":"Weapon","bEquip":true}]),
        json!([]),
        farm_quest_server,
    );
    let (result, logged, _) = fake.run(
        r#"
using Skua.Core.Interfaces;
public class Equipped {
    public void ScriptMain(IScriptInterface bot) {
        bot.Quests.EnsureAccept(11);
        bot.Log($"can complete {bot.Quests.CanComplete(11)}, turn-ins {bot.Flash.CallGameFunction<int>("world.maximumQuestTurnIns", 11)}");
    }
}
"#,
    );

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"can complete True, turn-ins 0".to_string()),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn maximum_turn_ins_counts_like_the_client() {
    let fake = Fake::start_with(
        FARM,
        json!([{"ItemID":87,"CharItemID":9,"sName":"Treeant Branch","iQty":7,"iStk":10,"sType":"Item","sES":"None"}]),
        json!([]),
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{
                    "21":{"QuestID":21,"sName":"Nothing Needed","bOnce":false,"iSlot":-1,"turnin":[],"oItems":{},"oRewards":{},"reward":[]},
                    "22":{"QuestID":22,"sName":"Once","bOnce":true,"iSlot":-1,"turnin":[{"ItemID":87,"iQty":2}],"oItems":{"87":{"ItemID":87,"sName":"Treeant Branch","iStk":10}},"oRewards":{},"reward":[]}}}"#,
                );
            }
        },
    );
    let (result, logged, sent) = fake.run(
        r#"
using Skua.Core.Interfaces;
public class Maximum {
    public void ScriptMain(IScriptInterface bot) {
        bot.Quests.Load(21, 22);
        bot.Wait.ForTrue(() => bot.Quests.Tree.Count == 2, 20);
        int Most(int id) => bot.Flash.CallGameFunction<int>("world.maximumQuestTurnIns", id);
        bot.Log($"empty {Most(21)}, once {Most(22)}, unloaded {Most(404)}");
    }
}
"#,
    );

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"empty -1, once 3, unloaded 0".to_string()),
        "{logged:#?}"
    );
    assert!(!sent.iter().any(|m| m.contains("%404%")), "{sent:#?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn clearing_the_quest_tree_forgets_loaded_and_accepted_quests() {
    let fake = Fake::start(farm_quest_server);
    let (result, logged, sent) = fake.run(
        r#"
using System.Dynamic;
using Skua.Core.Interfaces;
public class ClearTree {
    public void ScriptMain(IScriptInterface bot) {
        bot.Quests.EnsureAccept(11);
        bot.Flash.SetGameObject("world.questTree", new ExpandoObject());
        bot.Log($"tree {bot.Quests.Tree.Count} in progress {bot.Quests.IsInProgress(11)}");
        bot.Quests.EnsureLoad(11);
        bot.Log($"accepted again {bot.Quests.EnsureAccept(11)}");
    }
}
"#,
    );

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"tree 0 in progress False".to_string()),
        "{logged:#?}"
    );
    assert!(
        logged.contains(&"accepted again True".to_string()),
        "{logged:#?}"
    );
    let count = |needle: &str| sent.iter().filter(|m| m.contains(needle)).count();
    assert_eq!(count("%getQuests%5%11%"), 2, "{sent:#?}");
    assert_eq!(count("%acceptQuest%5%11%"), 2, "{sent:#?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_story_kills_for_a_quest_and_turns_it_in() {
    let fake = Fake::start_config(
        FARM,
        json!([]),
        json!([]),
        json!({"strQuests":"0".repeat(100)}),
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    &QUEST_11.replace(r#""iSlot":-1,"iValue":0"#, r#""iSlot":3,"iValue":1"#),
                );
            } else if msg.starts_with("%xt%zm%tryQuestComplete%5%11%") {
                farm_quest_server(msg, wire);
                wire.object(r#"{"cmd":"updateQuest","iIndex":3,"iValue":1}"#);
            } else {
                farm_quest_server(msg, wire);
            }
        },
    );
    let (result, logged, sent) = fake.run(KILL_QUEST);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%11%")),
        "{sent:#?}"
    );
    assert!(
        logged.iter().any(|l| l.ends_with("holding 1 Twilly Twig")),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_story_picks_up_a_map_item_for_a_quest() {
    let fake = Fake::start_config(
        FARM,
        json!([]),
        json!([]),
        json!({"strQuests":"0".repeat(100)}),
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
    );
    let (result, logged, sent) = fake.run(MAP_ITEM_QUEST);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%getMapItem%5%55%")),
        "{sent:#?}"
    );
    assert!(
        logged.iter().any(|l| l.ends_with("quest 12 done True")),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_turns_in_registered_quests_while_it_hunts() {
    let fake = Fake::start(farm_quest_server);
    let (result, logged, sent) = fake.run(REGISTERED);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        sent.iter()
            .any(|m| m.starts_with("%xt%zm%tryQuestComplete%5%11%")),
        "{sent:#?}"
    );
    assert!(
        logged.iter().any(|l| l.ends_with("holding 1 Twilly Twig")),
        "{logged:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn map_items_wait_the_action_delay_between_pickups_like_skua() {
    let pickups = Arc::new(Mutex::new(Vec::new()));
    let observed = pickups.clone();
    let fake = Fake::start_with(FARM, json!([]), json!([]), move |msg, wire| {
        if msg.starts_with("%xt%zm%getMapItem%") {
            observed.lock().unwrap().push(Instant::now());
        }
        if msg.starts_with("%xt%zm%getMapItem%5%55%") {
            wire.object(r#"{"cmd":"addItems","items":{"900":{"ItemID":900,"sName":"Mirror Unlocked","iQty":1,"iStk":1,"bTemp":1}}}"#);
        } else if msg.starts_with("%xt%zm%getMapItem%5%56%") {
            wire.object(r#"{"cmd":"addItems","items":{"901":{"ItemID":901,"sName":"Mirror Entered","iQty":1,"iStk":1,"bTemp":1}}}"#);
        }
    });
    let (result, logged, _) = fake.run(TWO_MAP_ITEMS);
    let pickups = pickups.lock().unwrap();

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.iter().any(|l| l.ends_with("holding 1 and 1")),
        "{logged:#?}"
    );
    assert_eq!(pickups.len(), 2, "{pickups:?}");
    let gap = pickups[1] - pickups[0];
    assert!(
        gap >= Duration::from_millis(1800),
        "getMapItems 1000 ms sleep plus the 800 ms ActionDelay, got {gap:?}"
    );

    let direct = Arc::new(Mutex::new(Vec::new()));
    let observed = direct.clone();
    let (result, logs, _) = Fake::start(move |msg, _| {
        if msg.starts_with("%xt%zm%getMapItem%") {
            observed.lock().unwrap().push(Instant::now());
        }
    })
    .run(
        r#"
using Skua.Core.Interfaces;
public class Pickups {
    public void ScriptMain(IScriptInterface bot) {
        bot.Map.GetMapItem(55);
        bot.Map.GetMapItem(56);
    }
}
"#,
    );
    assert_eq!(result, Outcome::Finished, "{logs:?}");
    let direct = direct.lock().unwrap();
    assert_eq!(direct.len(), 2, "{direct:?}");
    assert!(
        direct[1] - direct[0] >= Duration::from_millis(1780),
        "{direct:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_class_quest_is_unavailable_until_the_class_has_the_points_and_its_accept_is_sent_once() {
    let fake = Fake::start_with(
        FARM,
        json!([{"ItemID":15652,"CharItemID":78,"sName":"Rogue","iQty":900,"iStk":1,"sType":"Class","sES":"ar","bEquip":true}]),
        json!([]),
        |msg, wire| {
            if msg.contains("%getQuests%") {
                wire.object(
                    r#"{"cmd":"getQuests","quests":{
                    "17":{"QuestID":17,"sName":"Rank Three","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,"iClass":15652,"iReqCP":3600,"iGold":500,"iExp":1200,
                          "turnin":[{"ItemID":900,"iQty":2}],"oItems":{"900":{"ItemID":900,"sName":"Spark","iStk":5,"bTemp":1}},"oRewards":{},"reward":[]},
                    "18":{"QuestID":18,"sName":"Rank Two","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,"iClass":15652,"iReqCP":900,
                          "turnin":[],"oItems":{},"oRewards":{},"reward":[]}}}"#,
                );
            } else if msg.starts_with("%xt%zm%acceptQuest%5%17%") {
                wire.object(r#"{"cmd":"acceptQuest","bSuccess":0,"QuestID":17,"msg":"Requirements not met"}"#);
            }
        },
    );
    let (result, logged, sent) = fake.run(CLASS_QUEST);

    assert!(result == Outcome::Finished, "{result:?}\n{logged:#?}");
    assert!(
        logged.contains(&"rank 3 quest available False, rank 2 quest available True".to_string()),
        "{logged:#?}"
    );
    assert!(
        logged.contains(
            &"needs class 15652 at 3600, pays 500 gold 1200 xp, spark is temp True".to_string()
        ),
        "{logged:#?}"
    );
    assert!(
        logged.contains(&"accepted the locked one True".to_string()),
        "{logged:#?}"
    );
    assert_eq!(
        sent.iter()
            .filter(|m| m.contains("%acceptQuest%5%17%"))
            .count(),
        1,
        "{sent:#?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn special_completion_and_lite_reaccept_apply_to_direct_completions() {
    let (result, logs, sent) = Fake::start(|msg, wire| {
        if msg.contains("%getQuests%") {
            wire.object(r#"{"cmd":"getQuests","quests":{"19":{"QuestID":19,"sName":"Repeat","bOnce":false,"iSlot":-1,"turnin":[],"oItems":{},"oRewards":{},"reward":[]}}}"#);
        } else if msg.contains("%acceptQuest%5%19%") {
            wire.object(r#"{"cmd":"acceptQuest","QuestID":19,"bSuccess":1}"#);
        } else if msg.contains("%tryQuestComplete%5%19%") {
            wire.object(r#"{"cmd":"ccqr","QuestID":19,"bSuccess":1}"#);
        }
    }).run(r#"
using Skua.Core.Interfaces;
public class Reaccept {
    public void ScriptMain(IScriptInterface bot) {
        bot.Options.ActionDelay = 0;
        bot.Quests.EnsureAccept(19);
        bot.Lite.ReacceptQuest = true;
        bot.Quests.Complete(19, special: true);
        bot.Wait.ForTrue(() => bot.Quests.IsInProgress(19), 20);
        bot.Log($"accepted again {bot.Quests.IsInProgress(19)}");
        bot.Lite.ReacceptQuest = false;
        bot.Log($"completed {bot.Quests.Complete(19)} absent {bot.Quests.Complete(19)}");
        bot.Quests.Complete(19, 1027);
    }
}
"#);
    assert_eq!(result, Outcome::Finished, "{logs:?}");
    assert!(
        sent.iter()
            .any(|m| m.contains("%tryQuestComplete%5%19%-1%true%1%wvz%")),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .any(|m| m.contains("%tryQuestComplete%5%19%1027%false%1%wvz%")),
        "{sent:?}"
    );
    assert!(logs.contains(&"accepted again True".into()), "{logs:?}");
    assert!(
        logs.contains(&"completed True absent True".into()),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn quest_loader_service_repairs_a_range_and_registered_quests_notify_and_cancel() {
    let (result, logs, _) = Fake::start(|msg, wire| {
        if msg.contains("%getQuests%") {
            wire.object(r#"{"cmd":"getQuests","quests":{"20":{"QuestID":20,"sName":"Cached","bOnce":false,"iSlot":-1,"turnin":[],"oItems":{},"oRewards":{},"reward":[]}}}"#);
        }
    }).run(r#"
using Skua.Core.Interfaces;
using CommunityToolkit.Mvvm.DependencyInjection;
using CommunityToolkit.Mvvm.Messaging;
using CommunityToolkit.Mvvm.Messaging.Messages;
public class QuestRepair {
    public void ScriptMain(IScriptInterface bot) {
        var loader = Ioc.Default.GetRequiredService<IQuestDataLoaderService>();
        string file = System.IO.Path.Combine(System.IO.Path.GetTempPath(), System.Guid.NewGuid() + ".json");
        try {
            var data = loader.UpdateRangeAsync(file, 20, 20, null, CancellationToken.None).GetAwaiter().GetResult();
            bot.Log($"repaired {data.Count} cached {bot.Quests.Cached.Count}");
        } finally { System.IO.File.Delete(file); }
        object recipient = new();
        int notices = 0;
        WeakReferenceMessenger.Default.Register<PropertyChangedMessage<IEnumerable<int>>>(recipient, (_, m) => { if (m.PropertyName == "Registered") notices++; });
        bot.Quests.RegisterQuests(404);
        bot.Sleep(100);
        var clock = System.Diagnostics.Stopwatch.StartNew();
        bot.Quests.UnregisterAllQuests();
        bot.Log($"notices {notices} bounded {clock.ElapsedMilliseconds < 2500}");
    }
}
"#);
    assert_eq!(result, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"repaired 1 cached 1".into()), "{logs:?}");
    assert!(logs.contains(&"notices 2 bounded True".into()), "{logs:?}");
}
