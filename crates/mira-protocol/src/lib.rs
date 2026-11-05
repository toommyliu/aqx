pub mod flex;
mod framer;
pub mod packet;

pub use framer::{FrameTooLong, Framer};

use std::fmt;
use std::io::Read;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::read::{DeflateDecoder, ZlibDecoder};
use quick_xml::events::{BytesStart, Event as XmlEvent};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AreaId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonMapId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CharItemId(pub i64);

macro_rules! flex_id {
    ($name:ident) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                flex::int(d).map($name)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_i64(self.0)
            }
        }
    };
}

flex_id!(EntityId);
flex_id!(AreaId);
flex_id!(MonMapId);
flex_id!(MonId);
flex_id!(ItemId);
flex_id!(CharItemId);

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Xml(XmlMessage),
    Json(JsonMessage),
    Xt(XtMessage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlMessage {
    pub action: String,
    pub user: Option<XmlUser>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlUser {
    pub id: EntityId,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonMessage {
    pub cmd: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XtMessage {
    fields: Vec<String>,
}

impl XtMessage {
    pub fn cmd(&self) -> &str {
        &self.fields[2]
    }

    pub fn field(&self, index: usize) -> Option<&str> {
        self.fields.get(index).map(String::as_str)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("empty message")]
    Empty,
    #[error("compressed message is not valid base64")]
    Base64(#[from] base64::DecodeError),
    #[error("compressed message did not inflate")]
    Inflate(#[source] std::io::Error),
    #[error("message inflates past {MAX_MESSAGE_BYTES} bytes")]
    TooLarge,
    #[error("message is not UTF-8")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("unknown message kind {0:?}")]
    UnknownKind(char),
    #[error("XML message has no body action")]
    NoAction,
    #[error("XML message is malformed: {0}")]
    Xml(String),
    #[error("JSON message is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("JSON message has no b.o.cmd")]
    NoCmd,
    #[error("XT message is malformed")]
    MalformedXt,
}

pub fn unpack(frame: &[u8]) -> Result<String, DecodeError> {
    match frame.strip_prefix(b"Z") {
        Some(compressed) => inflate(&BASE64.decode(compressed)?),
        None => Ok(String::from_utf8(frame.to_vec())?),
    }
}

pub fn parse(text: &str) -> Result<Message, DecodeError> {
    match text.chars().next() {
        None => Err(DecodeError::Empty),
        Some('<') => decode_xml(text).map(Message::Xml),
        Some('{') => decode_json(text).map(Message::Json),
        Some('%') => decode_xt(text).map(Message::Xt),
        Some(other) => Err(DecodeError::UnknownKind(other)),
    }
}

fn inflate(compressed: &[u8]) -> Result<String, DecodeError> {
    let limit = MAX_MESSAGE_BYTES as u64 + 1;
    let mut out = Vec::new();
    if ZlibDecoder::new(compressed)
        .take(limit)
        .read_to_end(&mut out)
        .is_err()
    {
        out.clear();
        DeflateDecoder::new(compressed)
            .take(limit)
            .read_to_end(&mut out)
            .map_err(DecodeError::Inflate)?;
    }
    if out.len() > MAX_MESSAGE_BYTES {
        return Err(DecodeError::TooLarge);
    }
    Ok(String::from_utf8(out)?)
}

fn decode_xml(text: &str) -> Result<XmlMessage, DecodeError> {
    let malformed = |e: &dyn fmt::Display| DecodeError::Xml(e.to_string());
    let attr = |tag: &BytesStart, key: &str| -> Result<Option<String>, DecodeError> {
        tag.try_get_attribute(key)
            .map_err(|e| malformed(&e))?
            .map(|a| {
                a.unescape_value()
                    .map(|v| v.into_owned())
                    .map_err(|e| malformed(&e))
            })
            .transpose()
    };
    let mut reader = quick_xml::Reader::from_str(text);
    let mut action = None;
    let mut user: Option<XmlUser> = None;
    let mut in_name = false;
    loop {
        match reader.read_event().map_err(|e| malformed(&e))? {
            XmlEvent::Start(tag) | XmlEvent::Empty(tag) if tag.name().as_ref() == b"body" => {
                if action.is_none() {
                    action = Some(attr(&tag, "action")?.ok_or(DecodeError::NoAction)?);
                }
            }
            XmlEvent::Start(tag) | XmlEvent::Empty(tag) if user.is_none() => {
                let key = match tag.name().as_ref() {
                    b"u" => "i",
                    b"user" => "id",
                    _ => continue,
                };
                if let Some(id) = attr(&tag, key)?.and_then(|id| id.parse().ok()) {
                    user = Some(XmlUser {
                        id: EntityId(id),
                        name: None,
                    });
                }
            }
            XmlEvent::Start(tag) if tag.name().as_ref() == b"n" => {
                in_name = user.as_ref().is_some_and(|u| u.name.is_none());
            }
            XmlEvent::End(tag) if tag.name().as_ref() == b"n" => in_name = false,
            XmlEvent::Text(t) if in_name => {
                name_into(&mut user, &t.decode().map_err(|e| malformed(&e))?)
            }
            XmlEvent::CData(t) if in_name => {
                name_into(&mut user, &t.decode().map_err(|e| malformed(&e))?)
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    Ok(XmlMessage {
        action: action.ok_or(DecodeError::NoAction)?,
        user,
    })
}

fn name_into(user: &mut Option<XmlUser>, text: &str) {
    let text = text.trim();
    if let Some(user) = user
        && !text.is_empty()
    {
        user.name = Some(text.to_owned());
    }
}

fn decode_json(text: &str) -> Result<JsonMessage, DecodeError> {
    let mut root: Value = serde_json::from_str(text)?;
    let data = root
        .pointer_mut("/b/o")
        .map(Value::take)
        .ok_or(DecodeError::NoCmd)?;
    let cmd = data
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or(DecodeError::NoCmd)?
        .to_owned();
    Ok(JsonMessage { cmd, data })
}

fn decode_xt(text: &str) -> Result<XtMessage, DecodeError> {
    let fields: Vec<String> = text.split('%').map(str::to_owned).collect();
    if fields.len() < 4 || fields[1] != "xt" || fields[2].is_empty() {
        return Err(DecodeError::MalformedXt);
    }
    Ok(XtMessage { fields })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::{DeflateEncoder, ZlibEncoder};
    use serde_json::json;

    use super::*;

    fn decode(frame: &[u8]) -> Result<Message, DecodeError> {
        parse(&unpack(frame)?)
    }

    fn z(packed: Vec<u8>) -> Vec<u8> {
        [b"Z".as_slice(), BASE64.encode(packed).as_bytes()].concat()
    }

    fn compress(text: &str) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        z(encoder.finish().unwrap())
    }

    fn xt(text: &str) -> XtMessage {
        match decode(text.as_bytes()).unwrap() {
            Message::Xt(xt) => xt,
            other => panic!("expected XT, got {other:?}"),
        }
    }

    #[test]
    fn xml_message_exposes_its_body_action() {
        let message = decode(b"<msg t='sys'><body action='apiOK' r='0'></body></msg>").unwrap();
        assert_eq!(
            message,
            Message::Xml(XmlMessage {
                action: "apiOK".into(),
                user: None,
            })
        );
    }

    #[test]
    fn xml_message_exposes_the_user_it_names() {
        let entered = decode(
            b"<msg t='sys'><body action='uER' r='42'><u i='10' m='0' s='0' p='3'>\
              <n><![CDATA[player4]]></n><vars></vars></u></body></msg>",
        )
        .unwrap();
        let gone =
            decode(b"<msg t='sys'><body action='userGone' r='42'><user id='10' /></body></msg>")
                .unwrap();
        assert_eq!(
            entered,
            Message::Xml(XmlMessage {
                action: "uER".into(),
                user: Some(XmlUser {
                    id: EntityId(10),
                    name: Some("player4".into()),
                }),
            })
        );
        assert_eq!(
            gone,
            Message::Xml(XmlMessage {
                action: "userGone".into(),
                user: Some(XmlUser {
                    id: EntityId(10),
                    name: None,
                }),
            })
        );
    }

    #[test]
    fn json_message_exposes_cmd_and_the_o_object() {
        let message =
            decode(br#"{"t":"xt","b":{"r":-1,"o":{"cmd":"moveToArea","areaId":5}}}"#).unwrap();
        assert_eq!(
            message,
            Message::Json(JsonMessage {
                cmd: "moveToArea".into(),
                data: json!({"cmd": "moveToArea", "areaId": 5}),
            })
        );
    }

    #[test]
    fn xt_fields_use_the_split_index() {
        let message = xt("%xt%chatm%-1%zone~hello there%player1%");
        assert_eq!(message.cmd(), "chatm");
        assert_eq!(message.field(4), Some("zone~hello there"));
        assert_eq!(message.field(5), Some("player1"));
    }

    #[test]
    fn xt_keeps_empty_fields_in_place() {
        let message = xt("%xt%loginResponse%-1%0%-1%%Rejected.%");
        assert_eq!(message.field(6), Some(""));
        assert_eq!(message.field(7), Some("Rejected."));
    }

    #[test]
    fn compressed_messages_decode_like_plain_ones() {
        let text = r#"{"t":"xt","b":{"r":-1,"o":{"cmd":"loadInventoryBig","items":[]}}}"#;
        assert_eq!(
            decode(&compress(text)).unwrap(),
            decode(text.as_bytes()).unwrap()
        );
    }

    #[test]
    fn raw_deflate_messages_decode_too() {
        let text = "%xt%server%-1%hello%";
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(text.as_bytes()).unwrap();
        assert_eq!(
            decode(&z(encoder.finish().unwrap())).unwrap(),
            decode(text.as_bytes()).unwrap()
        );
    }

    #[test]
    fn compressed_messages_past_the_limit_are_rejected() {
        let text = format!("%xt%server%-1%{}%", "a".repeat(MAX_MESSAGE_BYTES));
        assert!(matches!(
            decode(&compress(&text)),
            Err(DecodeError::TooLarge)
        ));
    }

    #[test]
    fn malformed_messages_are_errors() {
        assert!(matches!(decode(b""), Err(DecodeError::Empty)));
        assert!(matches!(
            decode(b"hello"),
            Err(DecodeError::UnknownKind('h'))
        ));
        assert!(matches!(decode(b"%yt%a%1%"), Err(DecodeError::MalformedXt)));
        assert!(matches!(
            decode(br#"{"t":"xt","b":{"r":-1,"o":{}}}"#),
            Err(DecodeError::NoCmd)
        ));
        assert!(matches!(
            decode(b"<msg t='sys'><body r='0'></body></msg>"),
            Err(DecodeError::NoAction)
        ));
    }
}
