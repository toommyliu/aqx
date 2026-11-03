mod server;

use aqx_core::{ActError, Item, ItemRef};
use aqx_protocol::{CharItemId, ItemId, ShopId, ShopItemId};
use server::{FARM, Fake, Wire};

const SHOP: &str = r#"{"cmd":"loadShop","shopinfo":{"ShopID":1348,"sName":"Featured Gear Shop","items":[
    {"ItemID":102658,"ShopItemID":68071,"sName":"Sword","iCost":300,"bCoins":false,"bUpg":false,"iStk":1,"iQty":1,"sType":"Sword","sES":"Weapon"},
    {"ItemID":5,"ShopItemID":6,"sName":"Crown","iCost":900,"bCoins":true,"bUpg":false,"iStk":1,"iQty":1,"sType":"Helm","sES":"he"}]}}"#;

fn shopkeeper(msg: &str, wire: &mut Wire) {
    if msg.contains("%loadShop%5%1348%") {
        wire.object(SHOP);
    } else if msg.contains("%buyItem%5%102658%1348%68071%1%") {
        wire.object(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":102658,"CharItemID":77,"iQty":1,"bBank":0}"#);
    } else if msg.contains("%equipItem%5%102658%") {
        wire.object(r#"{"cmd":"equipItem","uid":1001,"ItemID":102658,"strES":"Weapon"}"#);
    } else if msg.contains("%bankToInv%5%1528%9%") {
        wire.object(r#"{"cmd":"bankToInv","ItemID":"1528"}"#);
    } else if msg.contains("%bankFromInv%5%1528%9%") {
        wire.object(r#"{"cmd":"buyItem","bitSuccess":0,"strMessage":"Not enough gold"}"#);
        wire.object(r#"{"cmd":"bankFromInv","ItemID":1528,"bSuccess":1}"#);
    }
}

fn makai_stack() -> Item {
    Item {
        id: ItemId(1528),
        char_item: Some(CharItemId(9)),
        name: "Defeated Makai".into(),
        qty: 49,
        max_stack: 50,
        kind: "Quest Item".into(),
        slot: "None".into(),
        equipped: false,
        ac: false,
        member: false,
        link: String::new(),
        meta: String::new(),
    }
}

#[test]
fn buy_then_equip_and_move_items_through_the_bank() {
    let mut fake = Fake::start(FARM, shopkeeper);
    fake.session.world.me.as_mut().unwrap().gold = 1000;
    fake.session.world.set_bank(Vec::new());
    let sword = ItemRef::Name("Sword".into());

    fake.session.buy(ShopId(1348), &sword, None, 1).unwrap();
    assert_eq!(fake.session.world.me.as_ref().unwrap().gold, 700);
    fake.session.buy(ShopId(1348), &sword, None, 1).unwrap();
    fake.session.equip(&sword).unwrap();
    assert!(fake.session.world.inventory.iter().any(|i| i.name == "Sword" && i.equipped));

    let crown = fake.session.buy(ShopId(1348), &ItemRef::Name("Crown".into()), None, 1);
    assert!(matches!(&crown, Err(ActError::Rejected(why)) if why.contains("900 AC")), "{crown:?}");

    fake.session.world.set_bank(vec![makai_stack()]);
    let makai = ItemRef::Name("Defeated Makai".into());
    fake.session.withdraw(&makai).unwrap();
    assert_eq!(fake.session.world.count_of(&makai), 49);
    fake.session.deposit(&makai).unwrap();
    assert_eq!(fake.session.world.count_of(&makai), 0);

    let mut empty = makai_stack();
    empty.qty = 0;
    fake.session.world.inventory.push(empty);
    fake.session.sell(&makai, 0).unwrap();

    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    assert_eq!(sent.iter().filter(|m| m.contains("%loadShop%")).count(), 1, "the shop was loaded once");
    assert!(!sent.iter().any(|m| m.contains("%buyItem%5%5%")), "the unaffordable crown was never requested");
    assert_eq!(sent.iter().filter(|m| m.contains("%buyItem%5%102658%")).count(), 1, "the owned sword was bought once");
}

const MERGE: &str = r#"{"cmd":"loadShop","shopinfo":{"ShopID":2000,"sName":"Merge Shop","items":[
    {"ItemID":102658,"ShopItemID":1,"sName":"Sword","iCost":0,"bCoins":false,"iStk":1,"iQty":1},
    {"ItemID":102658,"ShopItemID":2,"sName":"Sword","iCost":0,"bCoins":false,"iStk":1,"iQty":1}]}}"#;

#[test]
fn buying_picks_the_listing_and_reloading_a_cached_shop_makes_it_the_loaded_one() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%loadShop%5%2000%") {
            wire.object(MERGE);
        } else if msg.contains("%buyItem%5%102658%2000%2%1%") {
            wire.object(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":102658,"CharItemID":77,"iQty":1,"bBank":0}"#);
        } else {
            shopkeeper(msg, wire);
        }
    });
    fake.session.world.set_bank(Vec::new());

    fake.session.load_shop(ShopId(2000)).unwrap();
    fake.session.load_shop(ShopId(1348)).unwrap();
    fake.session.load_shop(ShopId(2000)).unwrap();
    assert_eq!(fake.session.world.loaded_shop().map(|s| s.id), Some(ShopId(2000)));

    fake.session.buy(ShopId(2000), &ItemRef::Id(ItemId(102658)), Some(ShopItemId(2)), 1).unwrap();
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    assert_eq!(sent.iter().filter(|m| m.contains("%loadShop%5%2000%")).count(), 1, "the cached shop was not requested again");
    assert!(sent.iter().any(|m| m.contains("%buyItem%5%102658%2000%2%1%")), "{sent:?}");
}
