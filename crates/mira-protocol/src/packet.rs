use std::fmt::Display;

use crate::{AreaId, EntityId};

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
