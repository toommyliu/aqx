use std::io::Write;

use crate::{AreaId, CharItemId, EntityId, ItemId, MonMapId, QuestId, ShopId, ShopItemId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aim {
    Monster(MonMapId),
    Player(EntityId),
}

pub enum Out<'a> {
    VerChk,
    Login { user: &'a str, version: &'a str, token: &'a str },
    FirstJoin,
    ClearIgnoreList,
    Join { user: &'a str, map: &'a str },
    MoveToCell { room: AreaId, cell: &'a str, pad: &'a str },
    RetrieveUserData { room: AreaId, eid: EntityId },
    RetrieveUserDatas { room: AreaId, others: &'a [EntityId] },
    RetrieveInventory { room: AreaId, eid: EntityId },
    Logout,
    Skill { counter: u32, slot: &'a str, targets: &'a [Aim] },
    Rest,
    Emote(&'a str),
    Respawn { room: AreaId, eid: EntityId },
    PickUp { room: AreaId, item: ItemId },
    Aggro { room: AreaId, monsters: &'a [MonMapId] },
    LoadQuests { room: AreaId, quests: &'a [QuestId] },
    AcceptQuest { room: AreaId, quest: QuestId },
    CompleteQuest { room: AreaId, quest: QuestId, reward: Option<ItemId>, times: u32 },
    Withdraw { room: AreaId, item: ItemId, char_item: CharItemId },
    Deposit { room: AreaId, item: ItemId, char_item: CharItemId },
    LoadShop { room: AreaId, shop: ShopId },
    Buy { room: AreaId, item: ItemId, shop: ShopId, shop_item: ShopItemId, qty: u32 },
    Sell { room: AreaId, item: ItemId, qty: u32, char_item: CharItemId },
    Equip { room: AreaId, item: ItemId },
    EquipConsumable { room: AreaId, link: &'a str, meta: &'a str, item: ItemId },
    UseItem { counter: u32, target: Aim, item: ItemId },
    ServerUseItem { item: ItemId },
    Afk(bool),
    Walk { room: AreaId, x: i32, y: i32, speed: u32 },
    Goto { name: &'a str },
    GetMapItem { room: AreaId, id: u32 },
    Raw(&'a str),
}

impl Out<'_> {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Out::VerChk => buf.extend_from_slice(b"<msg t='sys'><body action='verChk' r='0'><ver v='157' /></body></msg>"),
            Out::Login { user, version, token } => write!(
                buf,
                "<msg t='sys'><body action='login' r='0'><login z='zone_master'><nick><![CDATA[SPIDER#0001~{user}~{version}]]></nick><pword><![CDATA[{token}]]></pword></login></body></msg>"
            )
            .unwrap(),
            Out::FirstJoin => buf.extend_from_slice(b"%xt%zm%firstJoin%1%"),
            Out::ClearIgnoreList => buf.extend_from_slice(b"%xt%zm%cmd%1%ignoreList%$clearAll%"),
            Out::Join { user, map } => write!(buf, "%xt%zm%cmd%1%tfer%{user}%{map}%").unwrap(),
            Out::MoveToCell { room, cell, pad } => write!(buf, "%xt%zm%moveToCell%{room}%{cell}%{pad}%").unwrap(),
            Out::RetrieveUserData { room, eid } => write!(buf, "%xt%zm%retrieveUserData%{room}%{eid}%").unwrap(),
            Out::RetrieveUserDatas { room, others } => {
                write!(buf, "%xt%zm%retrieveUserDatas%{room}%").unwrap();
                for eid in *others {
                    write!(buf, "{eid}%").unwrap();
                }
            }
            Out::RetrieveInventory { room, eid } => write!(buf, "%xt%zm%retrieveInventory%{room}%{eid}%").unwrap(),
            Out::Logout => buf.extend_from_slice(b"%xt%zm%cmd%1%logout%"),
            Out::Skill { counter, slot, targets } => {
                write!(buf, "%xt%zm%gar%1%{counter}%").unwrap();
                for (i, aim) in targets.iter().enumerate() {
                    let sep = if i == 0 { "" } else { "," };
                    match aim {
                        Aim::Monster(id) => write!(buf, "{sep}{slot}>m:{id}").unwrap(),
                        Aim::Player(eid) => write!(buf, "{sep}{slot}>p:{eid}").unwrap(),
                    }
                }
                buf.extend_from_slice(b"%wvz%");
            }
            Out::Rest => buf.extend_from_slice(b"%xt%zm%restRequest%1%%"),
            Out::Emote(name) => write!(buf, "%xt%zm%emotea%1%{name}%").unwrap(),
            Out::Respawn { room, eid } => write!(buf, "%xt%zm%resPlayerTimed%{room}%{eid}%").unwrap(),
            Out::PickUp { room, item } => write!(buf, "%xt%zm%getDrop%{room}%{item}%").unwrap(),
            Out::Aggro { room, monsters } => {
                write!(buf, "%xt%zm%aggroMon%{room}%").unwrap();
                for id in *monsters {
                    write!(buf, "{id}%").unwrap();
                }
            }
            Out::LoadQuests { room, quests } => {
                write!(buf, "%xt%zm%getQuests%{room}%").unwrap();
                for id in *quests {
                    write!(buf, "{id}%").unwrap();
                }
            }
            Out::AcceptQuest { room, quest } => write!(buf, "%xt%zm%acceptQuest%{room}%{quest}%").unwrap(),
            Out::CompleteQuest { room, quest, reward, times } => {
                let reward = reward.map_or(-1, |id| i64::from(id.0));
                write!(buf, "%xt%zm%tryQuestComplete%{room}%{quest}%{reward}%false%{times}%wvz%").unwrap()
            }
            Out::Withdraw { room, item, char_item } => write!(buf, "%xt%zm%bankToInv%{room}%{item}%{char_item}%").unwrap(),
            Out::Deposit { room, item, char_item } => write!(buf, "%xt%zm%bankFromInv%{room}%{item}%{char_item}%").unwrap(),
            Out::LoadShop { room, shop } => write!(buf, "%xt%zm%loadShop%{room}%{shop}%").unwrap(),
            Out::Buy { room, item, shop, shop_item, qty } => write!(buf, "%xt%zm%buyItem%{room}%{item}%{shop}%{shop_item}%{qty}%").unwrap(),
            Out::Sell { room, item, qty, char_item } => write!(buf, "%xt%zm%sellItem%{room}%{item}%{qty}%{char_item}%").unwrap(),
            Out::Equip { room, item } => write!(buf, "%xt%zm%equipItem%{room}%{item}%").unwrap(),
            Out::EquipConsumable { room, link, meta, item } => write!(buf, "%xt%zm%geia%{room}%{link}%{meta}%{item}%").unwrap(),
            Out::UseItem { counter, target, item } => match target {
                Aim::Monster(id) => write!(buf, "%xt%zm%gar%1%{counter}%i1>m:{id}%{item}%wvz%").unwrap(),
                Aim::Player(eid) => write!(buf, "%xt%zm%gar%1%{counter}%i1>p:{eid}%{item}%wvz%").unwrap(),
            },
            Out::ServerUseItem { item } => write!(buf, "%xt%zm%serverUseItem%-1%+%{item}%").unwrap(),
            Out::Afk(on) => write!(buf, "%xt%zm%afk%1%{on}%").unwrap(),
            Out::Walk { room, x, y, speed } => write!(buf, "%xt%zm%mv%{room}%{x}%{y}%{speed}%").unwrap(),
            Out::Goto { name } => write!(buf, "%xt%zm%cmd%1%goto%{name}%").unwrap(),
            Out::GetMapItem { room, id } => write!(buf, "%xt%zm%getMapItem%{room}%{id}%").unwrap(),
            Out::Raw(text) => buf.extend_from_slice(text.as_bytes()),
        }
        buf.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(out: Out) -> String {
        let mut buf = Vec::new();
        out.encode(&mut buf);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn encodes_the_handshake_as_the_client_sends_it() {
        assert_eq!(encoded(Out::VerChk), "<msg t='sys'><body action='verChk' r='0'><ver v='157' /></body></msg>\0");
        assert_eq!(
            encoded(Out::Login { user: "hero", version: "5.31", token: "abc123" }),
            "<msg t='sys'><body action='login' r='0'><login z='zone_master'><nick><![CDATA[SPIDER#0001~hero~5.31]]></nick><pword><![CDATA[abc123]]></pword></login></body></msg>\0"
        );
        assert_eq!(encoded(Out::FirstJoin), "%xt%zm%firstJoin%1%\0");
        assert_eq!(encoded(Out::ClearIgnoreList), "%xt%zm%cmd%1%ignoreList%$clearAll%\0");
        assert_eq!(encoded(Out::Logout), "%xt%zm%cmd%1%logout%\0");
    }

    #[test]
    fn encodes_combat_and_upkeep_as_the_client_sends_them() {
        let room = AreaId(168500);
        assert_eq!(
            encoded(Out::Skill { counter: 14, slot: "aa", targets: &[Aim::Monster(MonMapId(48))] }),
            "%xt%zm%gar%1%14%aa>m:48%wvz%\0"
        );
        assert_eq!(
            encoded(Out::Skill { counter: 3, slot: "a2", targets: &[Aim::Monster(MonMapId(1)), Aim::Monster(MonMapId(2))] }),
            "%xt%zm%gar%1%3%a2>m:1,a2>m:2%wvz%\0"
        );
        assert_eq!(encoded(Out::Skill { counter: 0, slot: "a4", targets: &[Aim::Player(EntityId(1001))] }), "%xt%zm%gar%1%0%a4>p:1001%wvz%\0");
        assert_eq!(encoded(Out::Rest), "%xt%zm%restRequest%1%%\0");
        assert_eq!(encoded(Out::Emote("rest")), "%xt%zm%emotea%1%rest%\0");
        assert_eq!(encoded(Out::Respawn { room, eid: EntityId(1001) }), "%xt%zm%resPlayerTimed%168500%1001%\0");
        assert_eq!(encoded(Out::PickUp { room, item: ItemId(1528) }), "%xt%zm%getDrop%168500%1528%\0");
        assert_eq!(encoded(Out::Aggro { room, monsters: &[MonMapId(1), MonMapId(2)] }), "%xt%zm%aggroMon%168500%1%2%\0");
        assert_eq!(
            encoded(Out::UseItem { counter: 0, target: Aim::Player(EntityId(41361)), item: ItemId(11736) }),
            "%xt%zm%gar%1%0%i1>p:41361%11736%wvz%\0"
        );
        assert_eq!(encoded(Out::UseItem { counter: 4, target: Aim::Monster(MonMapId(3)), item: ItemId(9) }), "%xt%zm%gar%1%4%i1>m:3%9%wvz%\0");
        assert_eq!(encoded(Out::Afk(false)), "%xt%zm%afk%1%false%\0");
        assert_eq!(encoded(Out::Walk { room: AreaId(167964), x: 143, y: 465, speed: 8 }), "%xt%zm%mv%167964%143%465%8%\0");
        assert_eq!(encoded(Out::GetMapItem { room, id: 923 }), "%xt%zm%getMapItem%168500%923%\0");
        assert_eq!(encoded(Out::ServerUseItem { item: ItemId(18927) }), "%xt%zm%serverUseItem%-1%+%18927%\0");
    }

    #[test]
    fn encodes_quest_requests_as_the_client_sends_them() {
        let room = AreaId(167928);
        assert_eq!(
            encoded(Out::LoadQuests { room, quests: &[QuestId(11), QuestId(407)] }),
            "%xt%zm%getQuests%167928%11%407%\0"
        );
        assert_eq!(encoded(Out::AcceptQuest { room, quest: QuestId(11) }), "%xt%zm%acceptQuest%167928%11%\0");
        assert_eq!(
            encoded(Out::CompleteQuest { room: AreaId(167964), quest: QuestId(11), reward: None, times: 1 }),
            "%xt%zm%tryQuestComplete%167964%11%-1%false%1%wvz%\0"
        );
        assert_eq!(
            encoded(Out::CompleteQuest { room, quest: QuestId(445), reward: Some(ItemId(2481)), times: 5 }),
            "%xt%zm%tryQuestComplete%167928%445%2481%false%5%wvz%\0"
        );
    }

    #[test]
    fn encodes_bank_shop_and_equipment_requests_as_the_client_sends_them() {
        let room = AreaId(7111);
        let (item, char_item) = (ItemId(1528), CharItemId(77));
        assert_eq!(encoded(Out::Withdraw { room, item, char_item }), "%xt%zm%bankToInv%7111%1528%77%\0");
        assert_eq!(encoded(Out::Deposit { room, item, char_item }), "%xt%zm%bankFromInv%7111%1528%77%\0");
        assert_eq!(encoded(Out::LoadShop { room, shop: ShopId(1348) }), "%xt%zm%loadShop%7111%1348%\0");
        assert_eq!(
            encoded(Out::Buy { room, item: ItemId(102658), shop: ShopId(1348), shop_item: ShopItemId(68071), qty: 1 }),
            "%xt%zm%buyItem%7111%102658%1348%68071%1%\0"
        );
        assert_eq!(encoded(Out::Sell { room, item: ItemId(102658), qty: 1, char_item }), "%xt%zm%sellItem%7111%102658%1%77%\0");
        assert_eq!(encoded(Out::Equip { room, item: ItemId(95742) }), "%xt%zm%equipItem%7111%95742%\0");
        assert_eq!(
            encoded(Out::EquipConsumable { room, link: "potion", meta: "457", item: ItemId(11857) }),
            "%xt%zm%geia%7111%potion%457%11857%\0"
        );
    }

    #[test]
    fn encodes_join_time_requests_as_the_client_sends_them() {
        let room = AreaId(7111);
        assert_eq!(encoded(Out::Join { user: "hero", map: "yulgar" }), "%xt%zm%cmd%1%tfer%hero%yulgar%\0");
        assert_eq!(encoded(Out::Goto { name: "ally" }), "%xt%zm%cmd%1%goto%ally%\0");
        assert_eq!(encoded(Out::MoveToCell { room, cell: "Enter", pad: "Spawn" }), "%xt%zm%moveToCell%7111%Enter%Spawn%\0");
        assert_eq!(
            encoded(Out::RetrieveUserData { room, eid: EntityId(1001) }),
            "%xt%zm%retrieveUserData%7111%1001%\0"
        );
        assert_eq!(
            encoded(Out::RetrieveUserDatas { room, others: &[EntityId(1002), EntityId(1003)] }),
            "%xt%zm%retrieveUserDatas%7111%1002%1003%\0"
        );
        assert_eq!(
            encoded(Out::RetrieveInventory { room, eid: EntityId(1001) }),
            "%xt%zm%retrieveInventory%7111%1001%\0"
        );
    }
}
