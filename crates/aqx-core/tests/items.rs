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
        house: false,
        link: String::new(),
        file: String::new(),
        icon: String::new(),
        meta: String::new(),
        level: 0,
        enhancement: None,
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

#[test]
fn withdrawing_checks_the_inventory_the_item_goes_to() {
    let mut fake = Fake::start(FARM, shopkeeper);
    let sword = Item { id: ItemId(102658), name: "Sword".into(), kind: "Sword".into(), qty: 1, max_stack: 1, ..makai_stack() };
    let dust = |i: u32| Item { id: ItemId(2000 + i), name: format!("Dust {i}"), kind: "Resource".into(), ..makai_stack() };
    fake.session.world.inventory = std::iter::once(sword).chain((0..5).map(dust)).collect();
    fake.session.world.me.as_mut().unwrap().bag_slots = 1;
    fake.session.world.misc_slots = 6;
    let makai = ItemRef::Name("Defeated Makai".into());

    fake.session.world.set_bank(vec![makai_stack()]);
    fake.session.withdraw(&makai).unwrap();
    assert_eq!(fake.session.world.count_of(&makai), 49, "a full bag leaves room for quest items");

    fake.session.world.inventory.retain(|i| i.name != "Defeated Makai");
    fake.session.world.set_bank(vec![makai_stack()]);
    fake.session.world.misc_slots = 5;
    let refused = fake.session.withdraw(&makai);
    assert!(matches!(&refused, Err(ActError::Rejected(why)) if why == "the misc inventory is full"), "{refused:?}");

    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    assert_eq!(sent.iter().filter(|m| m.contains("%bankToInv%")).count(), 1);
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

#[test]
fn loading_a_shop_waits_out_combat_since_the_server_ignores_it_during_and_just_after_a_fight() {
    let mut calm_since: Option<std::time::Instant> = None;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") {
            calm_since = None;
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intState":2}},"m":{"2":{"intHP":800,"intState":2}}}"#);
        } else if msg.contains("%moveToCell%") {
            calm_since = Some(std::time::Instant::now());
            wire.object(r#"{"cmd":"uotls","unm":"hero","o":{"intState":1}}"#);
        } else if msg.contains("%loadShop%5%1348%") {
            match calm_since {
                Some(at) if at.elapsed() >= std::time::Duration::from_millis(1200) => wire.object(SHOP),
                Some(_) => wire.send("%xt%warning%-1%You cannot do that too soon after combat.%"),
                None => {}
            }
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    fake.session.use_skill(0, None).unwrap();
    let _ = fake.session.pump_until(Some(std::time::Instant::now() + std::time::Duration::from_millis(200)), |_| None::<()>);
    let started = std::time::Instant::now();
    fake.session.load_shop(ShopId(1348)).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn a_house_item_moves_between_the_bank_and_the_house_and_sells_from_it() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%bankToInv%5%70%22%") {
            wire.object(r#"{"cmd":"bankToInv","ItemID":"70"}"#);
        } else if msg.contains("%sellItem%5%70%1%22%") {
            wire.object(r#"{"cmd":"sellItem","CharItemID":22,"iQty":1,"intAmount":25,"bCoins":0}"#);
        }
    });
    let balloons = Item { id: ItemId(70), char_item: Some(CharItemId(22)), name: "Blue Balloons".into(), kind: "Floor Item".into(), qty: 1, max_stack: 1, house: true, ..makai_stack() };
    fake.session.world.me.as_mut().unwrap().house_slots = 5;
    fake.session.world.set_bank(vec![balloons]);
    let item = ItemRef::Name("Blue Balloons".into());

    fake.session.withdraw(&item).unwrap();
    assert_eq!((fake.session.world.house.len(), fake.session.world.count_of(&item)), (1, 1));
    fake.session.sell(&item, 1).unwrap();
    assert!(fake.session.world.house.is_empty());
}
