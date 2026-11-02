use serde::Deserialize;
use serde_json::value::RawValue;

use crate::EntityId;

#[derive(Debug)]
pub enum Message<'a> {
    Xml(Xml<'a>),
    Json(Json<'a>),
    Xt(Xt<'a>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    UnknownKind,
    Json(String),
    Xt,
}

impl<'a> Message<'a> {
    pub fn parse(text: &'a str) -> Result<Self, ParseError> {
        match text.as_bytes().first() {
            Some(b'<') => Ok(Message::Xml(Xml::parse(text))),
            Some(b'{') => Json::parse(text).map(Message::Json),
            Some(b'%') => Xt::parse(text).map(Message::Xt),
            Some(_) => Err(ParseError::UnknownKind),
            None => Err(ParseError::Empty),
        }
    }
}

#[derive(Debug)]
pub struct Xml<'a> {
    pub action: Option<&'a str>,
    pub text: &'a str,
}

impl<'a> Xml<'a> {
    fn parse(text: &'a str) -> Self {
        Xml { action: attribute(text, "action"), text }
    }

    pub fn user_ids(&self) -> impl Iterator<Item = EntityId> + 'a {
        let tag = if self.action == Some("userGone") { "<user id=" } else { "<u i=" };
        self.text.split(tag).skip(1).filter_map(|rest| {
            let rest = rest.strip_prefix(['\'', '"'])?;
            let end = rest.find(|c: char| !c.is_ascii_digit())?;
            rest[..end].parse().ok().map(EntityId)
        })
    }
}

fn attribute<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let (at, _) = text
        .match_indices(name)
        .find(|&(i, _)| text[..i].ends_with(' ') && text[i + name.len()..].starts_with('='))?;
    let rest = &text[at + name.len() + 1..];
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let value = &rest[1..];
    Some(&value[..value.find(quote)?])
}

#[derive(Debug)]
pub struct Json<'a> {
    pub cmd: &'a str,
    pub body: &'a RawValue,
}

impl<'a> Json<'a> {
    fn parse(text: &'a str) -> Result<Self, ParseError> {
        #[derive(Deserialize)]
        struct Envelope<'a> {
            #[serde(borrow)]
            b: Body<'a>,
        }
        #[derive(Deserialize)]
        struct Body<'a> {
            #[serde(borrow)]
            o: &'a RawValue,
        }
        #[derive(Deserialize)]
        struct Cmd<'a> {
            cmd: &'a str,
        }
        let json_err = |e: serde_json::Error| ParseError::Json(e.to_string());
        let body = serde_json::from_str::<Envelope>(text).map_err(json_err)?.b.o;
        let Cmd { cmd } = serde_json::from_str(body.get()).map_err(json_err)?;
        Ok(Json { cmd, body })
    }

    pub fn decode<T: Deserialize<'a>>(&self) -> serde_json::Result<T> {
        serde_json::from_str(self.body.get())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Xt<'a> {
    pub cmd: &'a str,
    pub room: &'a str,
    args: &'a str,
}

impl<'a> Xt<'a> {
    fn parse(text: &'a str) -> Result<Self, ParseError> {
        let rest = text.strip_prefix("%xt%").ok_or(ParseError::Xt)?;
        let rest = rest.strip_suffix('%').unwrap_or(rest);
        let mut fields = rest.splitn(3, '%');
        let cmd = fields.next().filter(|c| !c.is_empty()).ok_or(ParseError::Xt)?;
        let room = fields.next().unwrap_or("");
        let args = fields.next().unwrap_or("");
        Ok(Xt { cmd, room, args })
    }

    pub fn args(&self) -> impl Iterator<Item = &'a str> {
        let args = self.args;
        (!args.is_empty()).then(|| args.split('%')).into_iter().flatten()
    }

    pub fn arg(&self, i: usize) -> Option<&'a str> {
        self.args().nth(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_action_of_system_xml() {
        let Ok(Message::Xml(xml)) = Message::parse("<msg t='sys'><body action='apiOK' r='0'></body></msg>") else {
            panic!("not xml");
        };
        assert_eq!(xml.action, Some("apiOK"));
        let Ok(Message::Xml(xml)) = Message::parse("<cross-domain-policy><allow-access-from domain='*' /></cross-domain-policy>")
        else {
            panic!("not xml");
        };
        assert_eq!(xml.action, None);
    }

    #[test]
    fn lists_the_users_in_a_join() {
        let text = "<msg t='sys'><body action='joinOK' r='7111'><pid id='1'/><vars /><uLs r='7111'><u i='1002' m='0' s='0' p='1'><n><![CDATA[a]]></n><vars></vars></u><u i='1003' m='0' s='0' p='1'><n><![CDATA[b]]></n><vars></vars></u></uLs></body></msg>";
        let Ok(Message::Xml(xml)) = Message::parse(text) else { panic!("not xml") };
        assert_eq!(xml.user_ids().collect::<Vec<_>>(), [EntityId(1002), EntityId(1003)]);
    }

    #[test]
    fn reads_who_left() {
        let Ok(Message::Xml(xml)) = Message::parse("<msg t='sys'><body action='userGone' r='5'><user id='1003' /></body></msg>") else {
            panic!("not xml");
        };
        assert_eq!(xml.user_ids().collect::<Vec<_>>(), [EntityId(1003)]);
    }

    #[test]
    fn reads_the_cmd_and_body_of_json() {
        let text = r#"{"t":"xt","b":{"r":-1,"o":{"bSuccess":1,"QuestID":11,"msg":"success","cmd":"acceptQuest"}}}"#;
        let Ok(Message::Json(json)) = Message::parse(text) else { panic!("not json") };
        assert_eq!(json.cmd, "acceptQuest");
        #[derive(Deserialize)]
        struct Accept {
            #[serde(rename = "QuestID")]
            quest: u32,
            #[serde(rename = "bSuccess")]
            ok: u8,
        }
        let accept: Accept = json.decode().unwrap();
        assert_eq!((accept.quest, accept.ok), (11, 1));
    }

    #[test]
    fn rejects_json_without_a_cmd() {
        assert!(matches!(Message::parse(r#"{"t":"xt","b":{"r":-1,"o":{}}}"#), Err(ParseError::Json(_))));
    }

    #[test]
    fn splits_xt_strings_into_cmd_room_and_args() {
        let Ok(Message::Xt(xt)) = Message::parse("%xt%loginResponse%-1%true%1001%hero%Welcome!%") else {
            panic!("not xt");
        };
        assert_eq!((xt.cmd, xt.room), ("loginResponse", "-1"));
        assert_eq!(xt.args().collect::<Vec<_>>(), ["true", "1001", "hero", "Welcome!"]);
        let Ok(Message::Xt(xt)) = Message::parse("%xt%hi%-1%") else { panic!("not xt") };
        assert_eq!((xt.cmd, xt.arg(0)), ("hi", None));
        let Ok(Message::Xt(xt)) = Message::parse("%xt%zm%tryQuestComplete%167964%11%-1%false%1%wvz%") else {
            panic!("not xt");
        };
        assert_eq!((xt.cmd, xt.room, xt.arg(0)), ("zm", "tryQuestComplete", Some("167964")));
    }

    #[test]
    fn rejects_unknown_shapes() {
        assert_eq!(Message::parse("").unwrap_err(), ParseError::Empty);
        assert_eq!(Message::parse("hello").unwrap_err(), ParseError::UnknownKind);
        assert_eq!(Message::parse("%yz%cmd%").unwrap_err(), ParseError::Xt);
    }
}
