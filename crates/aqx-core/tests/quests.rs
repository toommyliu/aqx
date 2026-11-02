mod server;

use aqx_core::ActError;
use aqx_protocol::{ItemId, QuestId};
use server::{FARM, Fake};

const QUESTS: &str = r#"{"cmd":"getQuests","quests":{
    "11":{"QuestID":11,"sName":"Twilly's New Staff","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
          "turnin":[{"ItemID":87,"iQty":1}],"oItems":{"87":{"ItemID":87,"sName":"Treeant Branch"}},"reward":[{"ItemID":1027,"iQty":1,"iRate":100,"iType":0}]},
    "3577":{"QuestID":3577,"sName":"Re-Animation Station","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
          "turnin":[{"ItemID":24770,"iQty":1}],"oItems":{"24770":{"ItemID":24770,"sName":"Crystal of Reanimation"}},
          "oRewards":{"itemsC":{"0":{"ItemID":24771,"sName":"Deadly Intent Blade"},"1":{"ItemID":24772,"sName":"Can O Darkness"}}},
          "reward":[{"ItemID":24771,"iQty":1,"iRate":100,"iType":2},{"ItemID":24772,"iQty":1,"iRate":100,"iType":2}]}}}"#;

fn quest_server(msg: &str, wire: &mut server::Wire) {
    if msg.contains("%getQuests%") {
        wire.object(QUESTS);
    } else if let Some(rest) = msg.strip_prefix("%xt%zm%acceptQuest%5%") {
        let id = rest.trim_end_matches('%');
        wire.object(&format!(r#"{{"cmd":"acceptQuest","bSuccess":1,"QuestID":{id},"msg":"success"}}"#));
        let item = if id == "11" { 87 } else { 24770 };
        wire.object(&format!(r#"{{"cmd":"addItems","items":{{"{item}":{{"ItemID":{item},"sName":"Quest Item","bTemp":true,"iQty":1}}}}}}"#));
    } else if let Some(rest) = msg.strip_prefix("%xt%zm%tryQuestComplete%5%") {
        let id = rest.split('%').next().unwrap();
        wire.object(r#"{"cmd":"turnIn","sItems":"87:1"}"#);
        wire.object(&format!(r#"{{"cmd":"ccqr","bSuccess":1,"QuestID":{id},"sName":"Done","rewardObj":{{"intGold":100}}}}"#));
    }
}

#[test]
fn complete_accepts_first_then_turns_in_once_the_items_are_held() {
    let mut fake = Fake::start(FARM, quest_server);
    fake.session.complete(QuestId(11), None, 1).unwrap();
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    let at = |needle: &str| sent.iter().position(|m| m.contains(needle)).unwrap_or_else(|| panic!("never sent {needle}: {sent:#?}"));
    assert!(at("%getQuests%5%11%") < at("%acceptQuest%5%11%"));
    assert!(at("%acceptQuest%5%11%") < at("%tryQuestComplete%5%11%-1%false%1%wvz%"));
}

#[test]
fn a_choice_quest_needs_one_of_its_rewards_before_anything_is_sent() {
    let mut fake = Fake::start(FARM, quest_server);
    let refused = fake.session.complete(QuestId(3577), None, 1);
    assert!(matches!(&refused, Err(ActError::Rejected(why)) if why.contains("Deadly Intent Blade") && why.contains("Can O Darkness")), "{refused:?}");
    fake.session.complete(QuestId(3577), Some(ItemId(24772)), 1).unwrap();
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    assert_eq!(sent.iter().filter(|m| m.contains("%tryQuestComplete%")).collect::<Vec<_>>(), ["%xt%zm%tryQuestComplete%5%3577%24772%false%1%wvz%"]);
}
