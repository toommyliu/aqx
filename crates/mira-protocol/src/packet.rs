use std::fmt::{self, Display};

use crate::{
    AreaId, CharItemId, EntityId, ItemId, MapItemId, MonMapId, QuestId, ShopId, ShopItemId,
};

const CLIENT_TOKEN: &str = "SPIDER#0001";
const SMARTFOX_VERSION: &str = "157";

pub fn version_check() -> String {
    format!("<msg t='sys'><body action='verChk' r='0'><ver v='{SMARTFOX_VERSION}' /></body></msg>")
}

pub fn login(username: &str, client_version: &str, token: &str) -> String {
    format!(
        "<msg t='sys'><body action='login' r='0'><login z='zone_master'>\
         <nick><![CDATA[{CLIENT_TOKEN}~{username}~{client_version}]]></nick>\
         <pword><![CDATA[{token}]]></pword></login></body></msg>"
    )
}

pub fn first_join() -> String {
    zm::<&str>("firstJoin", 1, &[])
}

pub fn clear_ignore_list() -> String {
    zm("cmd", 1, &["ignoreList", "$clearAll"])
}

pub fn transfer(username: &str, map: &str) -> String {
    zm("cmd", 1, &["tfer", username, map])
}

pub fn transfer_to(room: AreaId, username: &str, map: &str, cell: &str, pad: &str) -> String {
    zm("cmd", room, &["tfer", username, map, cell, pad])
}

pub fn walk(room: AreaId, x: i32, y: i32, speed: u32) -> String {
    zm(
        "mv",
        room,
        &[x.to_string(), y.to_string(), speed.to_string()],
    )
}

pub fn goto(name: &str) -> String {
    zm("cmd", 1, &["goto", name])
}

pub fn move_to_cell(room: AreaId, cell: &str, pad: &str) -> String {
    zm("moveToCell", room, &[cell, pad])
}

pub fn retrieve_user_data(room: AreaId, eid: EntityId) -> String {
    zm("retrieveUserData", room, &[eid])
}

pub fn retrieve_user_datas(room: AreaId, eids: &[EntityId]) -> String {
    zm("retrieveUserDatas", room, eids)
}

pub fn retrieve_inventory(room: AreaId, eid: EntityId) -> String {
    zm("retrieveInventory", room, &[eid])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    Monster(MonMapId),
    Player(EntityId),
}

impl Display for Aim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Aim::Monster(id) => write!(f, "m:{id}"),
            Aim::Player(eid) => write!(f, "p:{eid}"),
        }
    }
}

pub fn gar(act: u8, slot: &str, aims: &[Aim]) -> String {
    let targets = if aims.is_empty() {
        format!("{slot}>")
    } else {
        aims.iter()
            .map(|aim| format!("{slot}>{aim}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    zm("gar", 1, &[act.to_string(), targets, "wvz".to_owned()])
}

pub fn gar_item(act: u8, slot: &str, aims: &[Aim], item: ItemId) -> String {
    let targets = aims
        .iter()
        .map(|aim| format!("{slot}>{aim}"))
        .collect::<Vec<_>>()
        .join(",");
    zm(
        "gar",
        1,
        &[act.to_string(), targets, item.to_string(), "wvz".to_owned()],
    )
}

pub fn geia(room: AreaId, link: &str, meta: &str, item: ItemId) -> String {
    zm(
        "geia",
        room,
        &[link.to_owned(), meta.to_owned(), item.to_string()],
    )
}

pub fn aggro_mon(room: AreaId, ids: &[MonMapId]) -> String {
    zm("aggroMon", room, ids)
}

pub fn res_player_timed(room: AreaId, eid: EntityId) -> String {
    zm("resPlayerTimed", room, &[eid])
}

pub fn get_quests(room: AreaId, ids: &[QuestId]) -> String {
    zm("getQuests", room, ids)
}

pub fn accept_quest(room: AreaId, id: QuestId) -> String {
    zm("acceptQuest", room, &[id])
}

pub fn try_quest_complete(room: AreaId, id: QuestId, reward: Option<ItemId>, times: u32) -> String {
    let reward = reward.map_or(-1, |item| item.0);
    zm(
        "tryQuestComplete",
        room,
        &[
            id.to_string(),
            reward.to_string(),
            "false".into(),
            times.to_string(),
            "wvz".into(),
        ],
    )
}

pub fn load_shop(room: AreaId, shop: ShopId) -> String {
    zm("loadShop", room, &[shop])
}

pub fn buy_item(room: AreaId, item: ItemId, shop: ShopId, listing: ShopItemId, qty: u32) -> String {
    zm(
        "buyItem",
        room,
        &[
            item.to_string(),
            shop.to_string(),
            listing.to_string(),
            qty.to_string(),
        ],
    )
}

pub fn sell_item(room: AreaId, item: ItemId, qty: u32, char_item: CharItemId) -> String {
    zm(
        "sellItem",
        room,
        &[item.to_string(), qty.to_string(), char_item.to_string()],
    )
}

pub fn bank_from_inv(room: AreaId, item: ItemId, char_item: CharItemId) -> String {
    zm(
        "bankFromInv",
        room,
        &[item.to_string(), char_item.to_string()],
    )
}

pub fn bank_to_inv(room: AreaId, item: ItemId, char_item: CharItemId) -> String {
    zm(
        "bankToInv",
        room,
        &[item.to_string(), char_item.to_string()],
    )
}

pub fn bank_swap_inv(
    room: AreaId,
    inv_item: ItemId,
    inv_char_item: CharItemId,
    bank_item: ItemId,
    bank_char_item: CharItemId,
) -> String {
    zm(
        "bankSwapInv",
        room,
        &[
            inv_item.to_string(),
            inv_char_item.to_string(),
            bank_item.to_string(),
            bank_char_item.to_string(),
        ],
    )
}

pub fn equip_item(room: AreaId, item: ItemId) -> String {
    zm("equipItem", room, &[item])
}

pub fn unequip_item(room: AreaId, item: ItemId) -> String {
    zm("unequipItem", room, &[item])
}

pub fn get_map_item(room: AreaId, item: MapItemId) -> String {
    zm("getMapItem", room, &[item])
}

pub fn get_drop(room: AreaId, item: ItemId) -> String {
    zm("getDrop", room, &[item])
}

pub fn rest_request() -> String {
    zm("restRequest", 1, &[""])
}

pub fn emote(name: &str) -> String {
    zm("emotea", 1, &[name])
}

pub fn logout() -> String {
    zm("cmd", 1, &["logout"])
}

fn zm<A: Display>(cmd: &str, room: impl Display, args: &[A]) -> String {
    let mut packet = format!("%xt%zm%{cmd}%{room}%");
    for arg in args {
        packet.push_str(&format!("{arg}%"));
    }
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_match_the_wire_format() {
        let room = AreaId(42);
        let eid = EntityId(7);
        assert_eq!(first_join(), "%xt%zm%firstJoin%1%");
        assert_eq!(clear_ignore_list(), "%xt%zm%cmd%1%ignoreList%$clearAll%");
        assert_eq!(
            move_to_cell(room, "Enter", "Spawn"),
            "%xt%zm%moveToCell%42%Enter%Spawn%"
        );
        assert_eq!(
            retrieve_user_data(room, eid),
            "%xt%zm%retrieveUserData%42%7%"
        );
        assert_eq!(
            retrieve_user_datas(room, &[EntityId(8), EntityId(9)]),
            "%xt%zm%retrieveUserDatas%42%8%9%"
        );
        assert_eq!(
            retrieve_inventory(room, eid),
            "%xt%zm%retrieveInventory%42%7%"
        );
        assert_eq!(logout(), "%xt%zm%cmd%1%logout%");
        assert_eq!(rest_request(), "%xt%zm%restRequest%1%%");
        assert_eq!(emote("rest"), "%xt%zm%emotea%1%rest%");
        assert_eq!(
            gar(
                3,
                "a1",
                &[Aim::Monster(MonMapId(5)), Aim::Monster(MonMapId(6))]
            ),
            "%xt%zm%gar%1%3%a1>m:5,a1>m:6%wvz%"
        );
        assert_eq!(
            gar(0, "a2", &[Aim::Player(eid)]),
            "%xt%zm%gar%1%0%a2>p:7%wvz%"
        );
        assert_eq!(gar(250, "a3", &[]), "%xt%zm%gar%1%250%a3>%wvz%");
        assert_eq!(
            gar_item(4, "i1", &[Aim::Player(eid)], ItemId(600)),
            "%xt%zm%gar%1%4%i1>p:7%600%wvz%"
        );
        assert_eq!(
            geia(room, "potion", "1044", ItemId(600)),
            "%xt%zm%geia%42%potion%1044%600%"
        );
        assert_eq!(
            aggro_mon(room, &[MonMapId(1), MonMapId(2)]),
            "%xt%zm%aggroMon%42%1%2%"
        );
        assert_eq!(res_player_timed(room, eid), "%xt%zm%resPlayerTimed%42%7%");
        assert_eq!(
            get_quests(room, &[QuestId(1), QuestId(2)]),
            "%xt%zm%getQuests%42%1%2%"
        );
        assert_eq!(accept_quest(room, QuestId(3)), "%xt%zm%acceptQuest%42%3%");
        assert_eq!(get_drop(room, ItemId(1027)), "%xt%zm%getDrop%42%1027%");
        assert_eq!(load_shop(room, ShopId(5)), "%xt%zm%loadShop%42%5%");
        assert_eq!(
            get_map_item(room, MapItemId(56)),
            "%xt%zm%getMapItem%42%56%"
        );
        assert_eq!(equip_item(room, ItemId(9)), "%xt%zm%equipItem%42%9%");
        assert_eq!(unequip_item(room, ItemId(9)), "%xt%zm%unequipItem%42%9%");
        assert_eq!(
            bank_from_inv(room, ItemId(1), CharItemId(2)),
            "%xt%zm%bankFromInv%42%1%2%"
        );
        assert_eq!(
            bank_to_inv(room, ItemId(1), CharItemId(2)),
            "%xt%zm%bankToInv%42%1%2%"
        );
        assert_eq!(
            bank_swap_inv(room, ItemId(1), CharItemId(2), ItemId(3), CharItemId(4)),
            "%xt%zm%bankSwapInv%42%1%2%3%4%"
        );
        assert_eq!(
            buy_item(room, ItemId(900), ShopId(5), ShopItemId(4005), 3),
            "%xt%zm%buyItem%42%900%5%4005%3%"
        );
        assert_eq!(
            sell_item(room, ItemId(101), 2, CharItemId(5001)),
            "%xt%zm%sellItem%42%101%2%5001%"
        );
        assert_eq!(
            try_quest_complete(room, QuestId(3), None, 1),
            "%xt%zm%tryQuestComplete%42%3%-1%false%1%wvz%"
        );
        assert_eq!(
            try_quest_complete(room, QuestId(3), Some(ItemId(900)), 5),
            "%xt%zm%tryQuestComplete%42%3%900%false%5%wvz%"
        );
        assert_eq!(
            transfer("player1", "yulgar-2"),
            "%xt%zm%cmd%1%tfer%player1%yulgar-2%"
        );
        assert_eq!(goto("player2"), "%xt%zm%cmd%1%goto%player2%");
        assert_eq!(walk(room, 667, 453, 8), "%xt%zm%mv%42%667%453%8%");
        assert_eq!(
            transfer_to(room, "player1", "xantown", "r7", "Left"),
            "%xt%zm%cmd%42%tfer%player1%xantown%r7%Left%"
        );
    }

    #[test]
    fn login_carries_the_name_version_and_token() {
        assert_eq!(
            login("player1", "5.31", "tok"),
            "<msg t='sys'><body action='login' r='0'><login z='zone_master'>\
             <nick><![CDATA[SPIDER#0001~player1~5.31]]></nick>\
             <pword><![CDATA[tok]]></pword></login></body></msg>"
        );
    }
}
