mod support;

use mira_script::Outcome;
use serde_json::json;
use support::{FARM, Fake};

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

const AURA_TIME: &str = r#"
using Skua.Core.Interfaces;

public class AuraTime
{
    public void ScriptMain(IScriptInterface bot)
    {
        var seal = bot.Self.GetAura("Righteous Seal");
        bot.Log($"seal stacks {bot.Self.GetAuraStacks("Righteous Seal")}, rule stacks {bot.Self.GetAuraRuleStacks("Righteous Seal")}");
        bot.Log($"seal x{seal?.Value} with {(seal?.RemainingTime is > 60 and <= 120 ? "minutes" : $"{seal?.RemainingTime} s")} left");
    }
}
"#;

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
        if (!Bot.Map.Cells.Contains("Blank")) throw new System.Exception("map cells missing");
        Bot.Map.Jump("r2", "Left");
        Core.JumpWait();
        Core.Logger($"cells {string.Join(",", Bot.Map.Cells)}, waited in {Bot.Player.Cell}");
        Core.SetOptions(false);
    }
}
"#;

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

const QUEST_11: &str = r#"{"cmd":"getQuests","quests":{"11":{"QuestID":11,"sName":"Twilly's New Staff","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
    "turnin":[{"ItemID":87,"iQty":1}],"oItems":{"87":{"ItemID":87,"sName":"Treeant Branch","iStk":10}},
    "oRewards":{"itemsS":{"0":{"ItemID":1027,"sName":"Twilly Twig","iStk":1}}},"reward":[{"ItemID":1027,"iQty":1,"iRate":100,"iType":0}]}}}"#;

fn assert_finished(outcome: Outcome, logs: &[String]) {
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn skua_kills_a_monster_and_hunts_it_for_a_temp_item() {
    let mut sap = 0;
    let (outcome, logs, sent) = Fake::start(move |message, wire| {
        if message.contains("%gar%") && message.contains(">m:2") {
            sap += 1;
            let act = message.split('%').nth(5).unwrap();
            wire.object(&format!(r#"{{"cmd":"ct","sarsa":[{{"actID":{act},"cInf":"p:1001","iRes":1,"a":[]}}],"m":{{"2":{{"intHP":0,"intState":0}}}}}}"#));
            wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
            wire.object(&format!(r#"{{"cmd":"addItems","items":{{"88":{{"ItemID":88,"sName":"Treeant Sap","iQty":1,"iQtyNow":{sap},"iStk":10,"bTemp":1}}}}}}"#));
            std::thread::sleep(std::time::Duration::from_millis(50));
            wire.object(r#"{"cmd":"mtls","id":2,"o":{"intHP":900,"intState":1}}"#);
        } else if message.contains("%moveToCell%5%r2%") {
            wire.object(r#"{"cmd":"mtls","id":2,"o":{"intHP":900,"intState":1}}"#);
        }
    }).run(KILL_AND_HUNT);
    assert_finished(outcome, &logs);
    assert!(logs.contains(&"killed".into()), "{logs:?}");
    assert!(logs.contains(&"holding 2 Treeant Sap".into()), "{logs:?}");
    assert!(sent.iter().any(|m| m.contains(">m:2")), "{sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_monster_in_combat_reads_as_state_2() {
    let (outcome, logs, _) = Fake::start(|message, wire| {
        if message.contains("%gar%") {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":800,"intState":2}}}"#);
        }
    })
    .run(TARGET_STATE);
    assert_finished(outcome, &logs);
    assert!(
        logs.contains(&"target Treeant in state 2".into()),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn an_aura_reports_the_time_it_has_left() {
    let source = AURA_TIME.replace("var seal", "bot.Map.Jump(\"r2\", \"Left\"); bot.Wait.ForTrue(() => bot.Self.HasActiveAura(\"Righteous Seal\"), 20); var seal");
    let (outcome, logs, _) = Fake::start(|message, wire| {
        if message.contains("%moveToCell%") {
            wire.object(r#"{"cmd":"aura+","auras":[{"nam":"Righteous Seal","dur":120,"val":7,"stk":2,"isNew":true}],"tInf":"p:1001"}"#);
        }
    }).run(&source);
    assert_finished(outcome, &logs);
    assert!(
        logs.contains(&"seal stacks 2, rule stacks 7".into()),
        "{logs:?}"
    );
    assert!(
        logs.contains(&"seal x7 with minutes left".into()),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn jump_wait_leaves_the_monster_cell_for_one_the_map_file_names() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(JUMP_WAIT);
    assert_finished(outcome, &logs);
    assert!(
        logs.iter()
            .any(|l| l.ends_with("cells Enter,r1,r2,Side1,Blank, waited in Side1")),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_kills_a_monster_in_a_named_cell_for_an_item() {
    let (outcome, logs, sent) = Fake::start(|message, wire| {
        if message.contains("%getQuests%") {
            wire.object(QUEST_11);
        } else if message.contains("%acceptQuest%5%11%") {
            wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#);
        } else if message.contains("%gar%") && message.contains(">m:2") {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
            wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
            wire.object(r#"{"cmd":"dropItem","items":{"87":{"ItemID":87,"sName":"Treeant Branch","iQty":1,"iStk":10}}}"#);
        } else if message.contains("%getDrop%5%87%") {
            wire.object(r#"{"cmd":"getDrop","bSuccess":1,"ItemID":87,"CharItemID":9,"iQty":1,"iQtyNow":1,"bBank":0}"#);
        }
    }).run(KILL_MONSTER);
    assert_finished(outcome, &logs);
    assert!(
        sent.iter().any(|m| m.contains("%moveToCell%5%r2%Left%")),
        "{sent:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("branches 1 from")),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_pickup_the_server_ignores_does_not_hold_up_the_script() {
    let source = PICKUP.replace(
        "Stopwatch clock",
        "bot.Map.Jump(\"r2\", \"Left\"); bot.Wait.ForDrop(\"Treeant Branch\"); Stopwatch clock",
    );
    let (outcome, logs, sent) = Fake::start(|message, wire| {
        if message.contains("%moveToCell%") {
            wire.object(r#"{"cmd":"dropItem","items":{"87":{"ItemID":87,"sName":"Treeant Branch","iQty":1,"iStk":10}}}"#);
        }
    }).run(&source);
    assert_finished(outcome, &logs);
    assert!(logs.contains(&"pickup returned".into()), "{logs:?}");
    assert!(
        sent.iter().any(|m| m.contains("%getDrop%5%87%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn skua_boosts_use_a_boost_item_until_the_boost_is_running() {
    let items = json!([{"ItemID":22447,"CharItemID":42,"sName":"Daily Login Class Boost  (20 min)","iQty":15,"iStk":99,"sType":"ServerUse","sES":"None"}]);
    let (outcome, logs, sent) = Fake::start_with(FARM, items, json!([]), |message, wire| {
        if message.contains("%serverUseItem%") && message.contains("%+%22447%") {
            wire.object(r#"{"cmd":"cpboost","op":"+","iSecsLeft":1200,"bShowShop":false}"#);
        }
    })
    .run(BOOST);
    assert_finished(outcome, &logs);
    assert_eq!(
        sent.iter()
            .filter(|m| m.contains("%serverUseItem%"))
            .count(),
        1,
        "{sent:?}"
    );
    assert!(
        logs.contains(&"class boost 22447 active: True".into()),
        "{logs:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn rest_packets_and_aggro_all_monsters_send_in_the_background() {
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(BACKGROUND_OPTIONS);
    assert_finished(outcome, &logs);
    assert!(sent.iter().any(|m| m.contains("%restRequest%")), "{sent:?}");
    assert!(
        sent.iter()
            .filter(|m| m.contains("%aggroMon%5%1%2%"))
            .count()
            >= 3,
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn resting_sends_once_and_waits_for_full_only_briefly() {
    let map = FARM.replace("\"intHP\":3000", "\"intHP\":100");
    let (outcome, logs, sent) = Fake::start_with(&map, json!([]), json!([]), |_, _| {}).run(REST);
    assert_finished(outcome, &logs);
    assert!(
        logs.contains(&"rest returned, full rest gave up after 2 s".into()),
        "{logs:?}"
    );
    assert_eq!(
        sent.iter().filter(|m| m.contains("%restRequest%")).count(),
        2,
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn aura_helpers_use_player_names_thresholds_patterns_and_both_subjects() {
    let source = r#"
using Skua.Core.Interfaces;
using Skua.Core.Models.Auras;
using Newtonsoft.Json;
public class AuraHelpers {
    public void ScriptMain(IScriptInterface bot) {
        bot.Send.Packet("aura-update");
        bot.Wait.ForTrue(() => bot.Self.HasActiveAura("Positive"), 20);
        var u = bot.UltraBossHelper;
        bot.Log($"charges {u.AnalyzeChargeMechanics(bot.Self, "Positive", "Negative", " Reversed")}");
        bot.Log($"thresholds {u.CheckAuraThreshold(bot.Self,"Positive",3,"<")} {u.CheckAuraThreshold(bot.Self,"Positive",1,">")} {u.CheckAuraThreshold(bot.Self,"Positive",2,"<=")} {u.CheckAuraThreshold(bot.Self,"Positive",2,">=")} {u.CheckAuraThreshold(bot.Self,"Positive",2,"==")} {u.CheckAuraThreshold(bot.Self,"Positive",3,"!=")}");
        try { u.CheckAuraThreshold(bot.Self,"Positive",2,"bad"); } catch (ArgumentException) { bot.Log("invalid comparison"); }
        bot.Log($"matches {u.GetAurasMatchingPattern(bot.Self.Auras,"positive").Count} {u.GetAurasMatchingPattern(bot.Self.Auras,"*Reversed").Count}");
        bot.Log($"skills {u.ShouldUseSkill(bot.Self,new() { ["Positive"] = n => n == 2 })} {u.ShouldUseSkill(bot.Target,new() { ["Missing"] = n => n == 444 })} {u.ShouldUseSkill(bot.Self,new() { ["Positive"] = n => n > 2 })}");
        bot.Log($"serialized {JsonConvert.DeserializeObject<List<Aura>>(bot.Self.GetPlayerAura(bot.Player.Username.ToUpperInvariant()))!.Count} {bot.Self.GetPlayerAura("missing")}");
        u.SetAttacksStopped(true);
        bot.Log($"stopped {bot.Combat.StopAttacking}, counter {u.IsCounterAttackActive}");
        u.SetAttacksStopped(false);
        bot.Log($"resumed {!bot.Combat.StopAttacking}");
    }
}
"#;
    let (outcome, logs, _) = Fake::start(|m,w| {
        if m == "aura-update" {
            w.object(r#"{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Positive","dur":60,"val":2,"isNew":true},{"nam":"Negative Reversed","dur":60,"isNew":true}]}"#);
        }
    }).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "charges (True, False, True)",
        "thresholds True True True True True True",
        "invalid comparison",
        "matches 1 1",
        "skills True True False",
        "serialized 2 []",
        "stopped True, counter False",
        "resumed True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn map_and_player_helpers_preserve_movement_and_safe_no_ops() {
    let source = r#"
using Skua.Core.Interfaces;
public class MovementHelpers {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"map file {bot.Map.FilePath} {bot.Map.FileName}");
        bot.Map.FilePath = @"maps\example.swf";
        bot.Log($"custom file {bot.Map.FileName}");
        bot.Map.FilePath = "";
        bot.Log($"empty file {bot.Map.FileName.Length}");
        bot.Map.Jump("r2", "Left", false, true);
        bot.Player.WalkTo(200,300,24);
        bot.Player.Goto("PLAYER1");
        bot.Combat.ApproachTarget();
        bot.Map.Reload();
        bot.Drops.RejectAll();
        bot.Drops.RejectAll(true);
        bot.Drops.RejectExcept("Dust");
        bot.Drops.RejectExcept(5);
        bot.Servers.SetLoginInfo("player2", "placeholder");
        bot.Servers.LastIP = "example.invalid";
        bot.Log($"last host {bot.Servers.LastIP}, password absent {bot.Player.Password is null}");
        bot.Log($"pvp {bot.Combat.AttackPlayer("player2")}, cell {bot.Player.Cell}");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "map file farm/farm.swf farm.swf",
        "custom file example.swf",
        "empty file 0",
        "last host example.invalid, password absent True",
        "pvp False, cell r2",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    assert!(
        !sent.iter().any(|p| p.contains("%moveToCell%5%r2%")),
        "{sent:?}"
    );
    assert!(sent.contains(&"%xt%zm%mv%5%200%300%8%".into()), "{sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn the_skill_timer_casts_class_skills_with_loaded_player_skills() {
    let source = r#"
using Skua.Core.Interfaces;
public class ClassSkills {
    public void ScriptMain(IScriptInterface bot) {
        bot.Map.Jump("r2", "Left");
        bot.Combat.Attack("Treeant");
        bot.Skills.StartAdvanced("1", 100, Skua.Core.Models.Skills.SkillUseMode.UseIfAvailable);
        bot.Sleep(500);
        bot.Skills.Stop();
        bot.Log("finished rotation");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start(|m, w| {
        if m.contains("%gar%") {
            let act = m.split('%').nth(5).unwrap();
            w.object(&format!(
                r#"{{"cmd":"ct","sarsa":[{{"actID":{act},"cInf":"p:1001","iRes":1,"a":[]}}]}}"#
            ));
        }
    })
    .run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        sent.iter()
            .any(|p| p.contains("%gar%") && p.contains("a1>")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn map_cells_fall_back_to_the_current_cell_after_a_failed_load() {
    let mut map: serde_json::Value = serde_json::from_str(FARM).unwrap();
    map["strMapFileName"] = serde_json::json!("missing.swf");
    let (outcome, logs, _) = Fake::start_with(
        &map.to_string(),
        serde_json::json!([]),
        serde_json::json!([]),
        |_, _| {},
    )
    .run(
        r#"
        using Skua.Core.Interfaces;
        public class Cells {
            public void ScriptMain(IScriptInterface bot) {
                bot.Log($"cells {string.Join(",", bot.Map.Cells)}");
            }
        }
    "#,
    );
    assert_finished(outcome, &logs);
    assert!(logs.iter().any(|line| line == "cells Enter"), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn attacks_choose_living_lowest_hp_then_map_id_and_accept_base_ids() {
    let mut map: serde_json::Value = serde_json::from_str(FARM).unwrap();
    map["mondef"][1]["sRace"] = json!("Chaos");
    map["mondef"][1]["strMonFileName"] = json!("treeant.swf");
    map["monmap"] = json!([
        {"MonMapID":9,"MonID":12,"strFrame":"r2"},
        {"MonMapID":7,"MonID":12,"strFrame":"r2"},
        {"MonMapID":6,"MonID":12,"strFrame":"r2"},
        {"MonMapID":5,"MonID":12,"strFrame":"r2"},
        {"MonMapID":4,"MonID":12,"strFrame":"r1"}
    ]);
    map["monBranch"] = json!([
        {"MonMapID":9,"MonID":12,"intHP":900,"intHPMax":900,"intState":1},
        {"MonMapID":7,"MonID":12,"intHP":200,"intHPMax":900,"intState":1},
        {"MonMapID":6,"MonID":12,"intHP":200,"intHPMax":900,"intState":1},
        {"MonMapID":5,"MonID":12,"intHP":0,"intHPMax":900,"intState":0},
        {"MonMapID":4,"MonID":12,"intHP":1,"intHPMax":900,"intState":1}
    ]);
    let (outcome, logs, _) = Fake::start_with(&map.to_string(), json!([]), json!([]), |_, _| {})
        .run(
            r#"
        using Skua.Core.Interfaces;
        public class TargetOrder {
            public void ScriptMain(IScriptInterface bot) {
                bot.Map.Jump("r2", "Left");
                foreach (string name in new[] {"tReE", "*", ""}) {
                    bot.Log($"name {bot.Combat.Attack(name)} {bot.Player.Target?.MapID}");
                    bot.Combat.CancelTarget();
                }
                bot.Log($"base {bot.Combat.Attack(12)} {bot.Player.Target?.MapID}");
                bot.Combat.CancelTarget();
                bot.Log($"map {bot.Combat.Attack(9)} {bot.Player.Target?.MapID}");
                bot.Log($"available {bot.Monsters.CurrentAvailableMonsters.Count}");
                bot.Log($"monster {bot.Monsters.CurrentMonsters[0].Race} {bot.Monsters.CurrentMonsters[0].FileName}");
                bot.Log($"dead {bot.Combat.Attack(new Skua.Core.Models.Monsters.Monster { MapID = 5 })} {bot.Player.HasTarget}");
                bot.Combat.CancelTarget();
            }
        }
    "#,
        );
    assert_finished(outcome, &logs);
    assert_eq!(
        logs.iter().filter(|s| *s == "name True 6").count(),
        3,
        "{logs:?}"
    );
    for expected in [
        "base True 6",
        "map True 9",
        "available 4",
        "monster Chaos treeant.swf",
        "dead True False",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn counter_handler_cancels_restores_and_resets_on_death_and_stop() {
    let (outcome, logs, _) = Fake::start(|m,w| {
        match m {
            "counter-on" => w.object(r#"{"cmd":"ct","anims":[{"cInf":"m:2","msg":"Treeant prepares a counter attack!!","animStr":"Charge"}]}"#),
            "counter-off" => w.object(r#"{"cmd":"aura-","tInf":"m:2","aura":{"nam":"Counter Attack"}}"#),
            "die" => w.object(r#"{"cmd":"ct","p":{"player1":{"intHP":0,"intState":0}}}"#),
            _ => {}
        }
    }).run(r#"
        using Skua.Core.Interfaces;
        public class Counters {
            public void ScriptMain(IScriptInterface bot) {
                bot.Map.Jump("r2", "Left");
                bot.Combat.EnableCounterHandler = true;
                bot.Combat.Attack("Treeant");
                bot.Send.Packet("counter-on");
                bot.Sleep(200);
                bot.Log($"held {bot.Combat.StopAttacking} {bot.Player.HasTarget}");
                bot.Send.Packet("counter-off");
                bot.Sleep(200);
                bot.Log($"restored {!bot.Combat.StopAttacking} {bot.Player.Target?.MapID}");
                bot.Combat.StopAttacking = true;
                bot.Send.Packet("die");
                bot.Sleep(200);
                bot.Log($"death reset {!bot.Combat.StopAttacking}");
                bot.Combat.StopAttacking = true;
                try { bot.Stop(false); } catch (OperationCanceledException) { bot.Log($"stop reset {!bot.Combat.StopAttacking}"); }
            }
        }
    "#);
    assert_eq!(outcome, Outcome::Stopped(None), "{logs:?}");
    for expected in [
        "held True False",
        "restored True 2",
        "death reset True",
        "stop reset True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn the_target_survives_a_death_until_the_respawn_move() {
    let (outcome, logs, _) = Fake::start(|m, w| match m {
        "die" => w.object(r#"{"cmd":"ct","p":{"player1":{"intHP":0,"intState":0}}}"#),
        "revive" => {
            w.object(r#"{"cmd":"uotls","unm":"player1","o":{"intHP":3000,"intState":1}}"#);
            w.send("%xt%resTimed%-1%r2%Left%");
        }
        _ => {}
    })
    .run(
        r#"
        using Skua.Core.Interfaces;
        public class DeathTarget {
            public void ScriptMain(IScriptInterface bot) {
                bot.Map.Jump("r2", "Left");
                bot.Combat.Attack("Treeant");
                bot.Send.Packet("die");
                bot.Sleep(200);
                bot.Log($"dead {bot.Player.HasTarget} {bot.Combat.Attack("Treeant")} {bot.Player.Target?.MapID}");
                bot.Send.Packet("revive");
                bot.Sleep(300);
                bot.Log($"respawned {bot.Player.Cell} {bot.Player.HasTarget}");
            }
        }
    "#,
    );
    assert_finished(outcome, &logs);
    for expected in ["dead True True 2", "respawned r2 False"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn skill_readiness_checks_targets_and_reports_real_cooldown_deadlines() {
    let (outcome, logs, _) = Fake::start(|m,w| {
        if m.contains("%gar%") {
            let act = m.split('%').nth(5).unwrap();
            w.object(&format!(r#"{{"cmd":"ct","sarsa":[{{"actID":{act},"cInf":"p:1001","iRes":1,"a":[]}}]}}"#));
        }
    }).run(r#"
        using Skua.Core.Interfaces;
        public class SkillReadiness {
            public void ScriptMain(IScriptInterface bot) {
                bot.Log($"target checks {bot.Skills.CanUseSkill(1)} {bot.Flash.Call<bool>("canUseSkill",1,true)} {bot.Skills.CanUseSkill(2)} {bot.Flash.Call<bool>("canUseSkill",5,true)}");
                bot.Skills.UseSkill(2);
                bot.Log($"cooldowns {bot.Skills.GlobalCooldownRemaining() > 0} {bot.Skills.SkillCooldownRemaining(1) > 0} {bot.Skills.SkillCooldownRemaining(2) > bot.Skills.GlobalCooldownRemaining()}");
                bot.Sleep(1700);
                bot.Log($"elapsed {bot.Skills.GlobalCooldownRemaining() == 0} {bot.Flash.Call<bool>("canUseSkill",1,true)} {!bot.Skills.CanUseSkill(2)}");
            }
        }
    "#);
    assert_finished(outcome, &logs);
    for expected in [
        "target checks False True True False",
        "cooldowns True True True",
        "elapsed True True True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn action_cooldown_waits_until_the_engine_lock_expires() {
    let (outcome, logs, _) = Fake::start(|_,_| {}).run(r#"
        using System.Diagnostics;
        using Skua.Core.Interfaces;
        using Skua.Core.Models;
        public class ActionLocks {
            public void ScriptMain(IScriptInterface bot) {
                bot.Player.Rest();
                bot.Log($"locked {!bot.Wait.IsActionAvailable(GameActions.Rest)} {!bot.Wait.IsActionAvailable("rest")} {bot.Wait.IsActionAvailable("unknown")}");
                var watch = Stopwatch.StartNew();
                bool available = bot.Wait.ForActionCooldown(GameActions.Rest);
                bot.Log($"waited {available && watch.ElapsedMilliseconds >= 1500} {bot.Wait.IsActionAvailable(GameActions.Rest)}");
            }
        }
    "#);
    assert_finished(outcome, &logs);
    for expected in ["locked True True False", "waited True True"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn aura_queries_include_other_players_and_choose_the_last_named_monster() {
    let mut map: serde_json::Value = serde_json::from_str(FARM).unwrap();
    map["uoBranch"].as_array_mut().unwrap().push(json!({"entID":1002,"strUsername":"player2","strFrame":"r2","strPad":"Left","intHP":100,"intHPMax":100,"intMP":100,"intMPMax":100,"intState":1}));
    map["monmap"]
        .as_array_mut()
        .unwrap()
        .push(json!({"MonMapID":3,"MonID":12,"strFrame":"r2"}));
    map["monBranch"]
        .as_array_mut()
        .unwrap()
        .push(json!({"MonMapID":3,"MonID":12,"intHP":900,"intHPMax":900,"intState":1}));
    let (outcome, logs, _) = Fake::start_with(&map.to_string(), json!([]), json!([]), |m,w| {
        if m == "auras" {
            for target in ["p:1002", "p:1001", "m:3"] {
                w.object(&json!({"cmd":"aura+","tInf":target,"auras":[{"nam":"Ward","dur":60,"val":2,"stk":3,"icon":"ward-icon","desc":"Ward description","isNew":true}]}).to_string());
            }
        }
    }).run(r#"
        using Skua.Core.Interfaces;
        using Newtonsoft.Json;
        using Skua.Core.Models.Auras;
        public class AuraQueries {
            public void ScriptMain(IScriptInterface bot) {
                bot.Send.Packet("auras");
                bot.Wait.ForTrue(() => bot.Self.HasActiveAura("Ward"), 20);
                bot.Sleep(100);
                var other = JsonConvert.DeserializeObject<List<Aura>>(bot.Self.GetPlayerAura("PLAYER2"));
                var monster = JsonConvert.DeserializeObject<List<Aura>>(bot.Target.GetMonsterAura("TREEANT"));
                bot.Log($"auras {other?.Single().Name} {monster?.Single().Name}");
                bot.Log($"missing {bot.Target.GetMonsterAura(999)} {bot.Target.GetMonsterAura("missing")}");
                var snapshot = bot.Self.GetAuraSnapshot("Ward");
                bot.Log($"snapshot {snapshot?.Icon} {snapshot?.Description}");
            }
        }
    "#);
    assert_finished(outcome, &logs);
    for expected in [
        "auras Ward Ward",
        "missing Error: Couldn't get Monster Object Tree Error: Couldn't get Monster Object Tree",
        "snapshot ward-icon Ward description",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn cancelling_auto_attack_preserves_the_target_and_approach_resumes_it() {
    let mut map: serde_json::Value = serde_json::from_str(FARM).unwrap();
    map["uoBranch"].as_array_mut().unwrap().push(json!({"entID":1002,"strUsername":"player2","strFrame":"r1","intHP":100,"intHPMax":100,"intState":1}));
    let (outcome, logs, _) = Fake::start_with(&map.to_string(), json!([]), json!([]), |m,w| {
        if m.contains("%gar%") {
            let act = m.split('%').nth(5).unwrap();
            w.object(&format!(r#"{{"cmd":"ct","sarsa":[{{"actID":{act},"cInf":"p:1001","iRes":1,"a":[]}}]}}"#));
        }
    }).run(r#"
        using Skua.Core.Interfaces;
        public class CombatSelection {
            public void ScriptMain(IScriptInterface bot) {
                bot.Map.Jump("r2", "Left");
                bot.Combat.Attack("Treeant");
                bot.Combat.CancelAutoAttack();
                bot.Sleep(100);
                bot.Combat.UntargetSelf();
                bot.Log($"retained {bot.Player.Target?.MapID}");
                bot.Combat.ApproachTarget();
                bot.Log($"resumed {bot.Player.Target?.MapID}");
                bot.Combat.CancelTarget();
                bot.Log($"cleared {!bot.Player.HasTarget}");
                bot.Log($"remote {bot.Combat.AttackPlayer("player2")}");
                bot.Log($"self {bot.Combat.AttackPlayer(bot.Player.Username.ToUpperInvariant())} {bot.Player.HasTarget}");
                bot.Combat.UntargetSelf();
                bot.Log($"self cleared {!bot.Player.HasTarget}");
            }
        }
    "#);
    assert_finished(outcome, &logs);
    for expected in [
        "retained 2",
        "resumed 2",
        "cleared True",
        "remote False",
        "self True True",
        "self cleared True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn skill_info_preserves_action_metadata_and_last_use() {
    let (outcome, logs, _) = Fake::start(|m,w| {
        if m == "skill-info" {
            w.object(r#"{"cmd":"sAct","actions":{"active":[{"ref":"a1","nam":"Strike","cd":3000,"mp":10,"tgt":"s","tgtMin":1,"tgtMax":2,"id":7,"actID":9,"damage":1.5,"icon":"strike-icon","desc":"Strike description","range":301,"typ":"s","anim":"Cast,Attack","isOK":true}]}}"#);
        } else if m.contains("%gar%") {
            let act = m.split('%').nth(5).unwrap();
            w.object(&format!(r#"{{"cmd":"ct","sarsa":[{{"actID":{act},"cInf":"p:1001","iRes":1,"a":[]}}]}}"#));
        }
    }).run(r#"
        using Skua.Core.Interfaces;
        public class SkillDetails {
            public void ScriptMain(IScriptInterface bot) {
                bot.Send.Packet("skill-info");
                bot.Wait.ForTrue(() => bot.Player.Skills?.Length == 1,20);
                var skill = bot.Player.Skills![0];
                bot.Log($"skill {skill.ID} {skill.ActID} {skill.Damage} {skill.Icon} {skill.Description} {skill.Range} {skill.Type} {skill.MinTargets} {string.Join(",",skill.Animations)}");
                using var active = bot.Flash.CreateFlashObject<object>("world.actions.active");
                using var entry = active.ToArray().Get(0);
                using var time = entry.GetChild<long>("ts");
                bot.Flash.CallGameFunction("world.testAction", bot.Flash.GetArrayObject<System.Dynamic.ExpandoObject>("world.actions.active", 0)!);
                bot.Log($"last use {bot.Player.Skills[0].LastUse > 0}");
                bot.Log($"linked {time.Value > 0}");
                time.Value = 0;
                bot.Sleep(1700);
                bot.Log($"reset {bot.Skills.SkillCooldownRemaining(0) == 0} {bot.Player.Skills[0].LastUse == 0}");
            }
        }
    "#);
    assert_finished(outcome, &logs);
    for expected in [
        "skill 7 9 1.5 strike-icon Strike description 301 s 1 Cast,Attack",
        "last use True",
        "linked True",
        "reset True True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn skill_set_updates_use_downloaded_contents_and_report_failures() {
    let (outcome, logs, _) = Fake::start(|_,_| {}).run(r#"
        using System.Net;
        using System.Net.Sockets;
        using System.Text;
        using CommunityToolkit.Mvvm.DependencyInjection;
        using Skua.Core.Interfaces;
        using Skua.Core.Interfaces.Services;
        using Skua.Core.Utils;
        public class SkillUpdates {
            public void ScriptMain(IScriptInterface bot) {
                var port = new TcpListener(IPAddress.Loopback, 0);
                port.Start();
                int number = ((IPEndPoint)port.LocalEndpoint).Port;
                port.Stop();
                using var server = new HttpListener();
                string url = $"http://localhost:{number}/";
                server.Prefixes.Add(url);
                server.Start();
                string file = ClientFileSources.SkuaAdvancedSkillsFile;
                string previous = File.ReadAllText(file);
                string content = previous + "\n ";
                var serving = Task.Run(async () => {
                    foreach (int status in new[] {200,500,500,500}) {
                        var context = await server.GetContextAsync();
                        context.Response.StatusCode = status;
                        byte[] bytes = Encoding.UTF8.GetBytes(content);
                        await context.Response.OutputStream.WriteAsync(bytes);
                        context.Response.Close();
                    }
                });
                HttpClients.GitHubRaw.BaseAddress = new Uri(url);
                try {
                    var service = Ioc.Default.GetRequiredService<IGetScriptsService>();
                    bool downloaded = service.UpdateSkillSetsFile().GetAwaiter().GetResult();
                    bot.Log($"downloaded {downloaded} {File.ReadAllText(file) == content}");
                    bot.Log($"failure {!service.UpdateSkillSetsFile().GetAwaiter().GetResult()} {File.ReadAllText(file) == content}");
                } finally { File.WriteAllText(file,previous); }
            }
        }
    "#);
    assert_finished(outcome, &logs);
    for expected in ["downloaded True True", "failure True True"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}
