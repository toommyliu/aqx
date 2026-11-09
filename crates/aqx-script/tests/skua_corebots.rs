mod support;

use aqx_script::Outcome;
use serde_json::json;
use support::{FARM, Fake};

#[test]
#[ignore = "run with npm run skua:test"]
fn core_bots_sets_its_options_up_and_down() {
    let source = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;
public class SetOptionsOnly {
    public void ScriptMain(IScriptInterface bot) {
        var core = CoreBots.Instance;
        core.SetOptions();
        core.Logger("options set");
        bot.Log($"bank loaded {bot.Bank.Loaded}, gems {bot.Bank.GetQuantity("Banked Gem")}");
        bot.Log($"class {bot.Player.CurrentClass?.Name}, tagged {bot.Accounts.HasTag("LR")}, skills running {bot.Skills.TimerRunning}");
        bot.Skills.Stop();
        core.SetOptions(false);
        bot.Log($"options restored {!bot.Options.SafeTimings && !bot.Options.InfiniteRange && !bot.Options.SkipCutscenes}");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start_with(FARM,
        json!([{"ItemID":100,"CharItemID":5000,"sName":"Legion Revenant","sType":"Class","sES":"ar","bEquip":1,"iQty":302500,"iStk":1}]),
        json!([{"ItemID":300,"CharItemID":9300,"sName":"Banked Gem","iQty":3,"iStk":10,"sType":"Item","bBank":true}]),
        |_, _| {}).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:#?}");
    assert!(logs.iter().any(|l| l.contains("options set")), "{logs:#?}");
    assert!(
        logs.iter().any(|l| l.contains("Script ran for")),
        "{logs:#?}"
    );
    assert!(
        logs.iter().any(|l| l == "options restored True"),
        "{logs:#?}"
    );
    assert!(
        logs.iter().any(|l| l == "bank loaded True, gems 3"),
        "{logs:#?}"
    );
    assert!(
        logs.iter()
            .any(|l| l == "class Legion Revenant, tagged False, skills running True"),
        "{logs:#?}"
    );
    assert!(!sent.iter().any(|p| p.contains("%loadBank%")), "{sent:#?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn auto_enhance_does_not_repurchase_the_applied_enhancement() {
    let source = r#"
//cs_include Scripts/CoreBots.cs
//cs_include Scripts/CoreAdvanced.cs
using Skua.Core.Interfaces;
public class EnhanceTwice {
    public void ScriptMain(IScriptInterface bot) {
        var adv = CoreAdvanced.Instance;
        adv.EnhanceEquipped(EnhancementType.Lucky, wSpecial: WeaponSpecial.Awe_Blast);
        var item = bot.Inventory.Items.First(i => i.ID == 100);
        bot.Log($"enhancement {item.EnhancementLevel} {item.EnhancementPatternID} {item.ProcID} {bot.Flash.GetGameObject<int>("world.invTree.100.iEnh")}");
        adv.EnhanceEquipped(EnhancementType.Lucky, wSpecial: WeaponSpecial.Awe_Blast);
        bot.Log($"gold {bot.Player.Gold}, coins {bot.Player.Coins}");
    }
}
"#;
    let (outcome, logs, sent) = Fake::start_config(
        FARM,
        json!([{"ItemID":100,"CharItemID":5000,"sName":"Sword","sType":"Sword","sES":"Weapon","bEquip":1,"iQty":1,"iStk":1,"EnhID":1,"EnhLvl":1,"EnhPatternID":1}]),
        json!([]),
        json!({"intLevel":100,"intGold":50000,"intCoins":30}),
        |message, wire| {
            if message.contains("%getQuests%5%2937%") {
                wire.object(r#"{"cmd":"getQuests","quests":{"2937":{"QuestID":2937,"sName":"Unlock Awe","bOnce":1,"iSlot":-1,"iValue":1}}}"#);
            } else if message.contains("%loadShop%5%639%") {
                wire.object(r#"{"cmd":"loadShop","shopinfo":{"ShopID":639,"sName":"Enhancements","Location":"Menu","items":[{"ItemID":40514,"ShopItemID":1,"sName":"Awe Blast","sType":"Enhancement","iLvl":70,"iCost":7000,"iQty":1,"iStk":1}]}}"#);
            } else if message.contains("%enhanceItemShop%5%100%40514%639%") {
                wire.object(r#"{"cmd":"enhanceItemShop","ItemIDs":[100],"EnhID":40514,"EnhPID":9,"EnhLvl":70,"EnhRty":6,"EnhDPS":100,"EnhRng":10,"ProcID":3,"iCost":7000,"bSuccess":1}"#);
            }
        },
    ).run(source);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert_eq!(
        sent.iter()
            .filter(|packet| packet.contains("%enhanceItemShop%"))
            .count(),
        1,
        "{sent:?}"
    );
    assert!(
        logs.contains(&"enhancement 70 9 3 40514".into()),
        "{logs:?}"
    );
    assert!(logs.contains(&"gold 43000, coins 30".into()), "{logs:?}");
}
