use std::io::Write;

use crate::{AreaId, EntityId, ItemId, MonMapId, QuestId};

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
    fn encodes_join_time_requests_as_the_client_sends_them() {
        let room = AreaId(7111);
        assert_eq!(encoded(Out::Join { user: "hero", map: "yulgar" }), "%xt%zm%cmd%1%tfer%hero%yulgar%\0");
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
