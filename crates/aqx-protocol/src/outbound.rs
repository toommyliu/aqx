use std::io::Write;

use crate::{AreaId, EntityId};

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
