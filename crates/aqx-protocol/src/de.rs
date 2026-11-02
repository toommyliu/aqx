use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Loose {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

pub fn int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    match Loose::deserialize(d)? {
        Loose::Bool(b) => Ok(i64::from(b)),
        Loose::Int(n) => Ok(n),
        Loose::Float(f) => Ok(f as i64),
        Loose::Text(s) => s.trim().parse().map_err(|_| serde::de::Error::custom(format!("{s:?} is not a number"))),
    }
}

pub fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match Option::<Loose>::deserialize(d)? {
        None => String::new(),
        Some(Loose::Text(s)) => s,
        Some(Loose::Int(n)) => n.to_string(),
        Some(Loose::Float(f)) => f.to_string(),
        Some(Loose::Bool(b)) => b.to_string(),
    })
}

pub fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(match Loose::deserialize(d)? {
        Loose::Bool(b) => b,
        Loose::Int(n) => n != 0,
        Loose::Float(f) => f != 0.0,
        Loose::Text(s) => !matches!(s.as_str(), "" | "0" | "false"),
    })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use crate::FactionId;

    #[derive(Deserialize)]
    struct Faction {
        #[serde(rename = "FactionID")]
        id: FactionId,
        #[serde(rename = "iRep", deserialize_with = "super::int")]
        rep: i64,
        #[serde(rename = "bUpg", deserialize_with = "super::flag")]
        member: bool,
    }

    #[test]
    fn reads_numbers_sent_as_strings_or_numbers() {
        let a: Faction = serde_json::from_str(r#"{"FactionID":"46","iRep":"302500","bUpg":"0"}"#).unwrap();
        let b: Faction = serde_json::from_str(r#"{"FactionID":46,"iRep":302500,"bUpg":1}"#).unwrap();
        assert_eq!((a.id, a.rep, a.member), (FactionId(46), 302500, false));
        assert_eq!((b.id, b.rep, b.member), (FactionId(46), 302500, true));
        assert!(serde_json::from_str::<Faction>(r#"{"FactionID":"x","iRep":1,"bUpg":0}"#).is_err());
    }

    #[test]
    fn reads_text_sent_as_null_or_a_number() {
        #[derive(Deserialize)]
        struct Named {
            #[serde(deserialize_with = "super::text")]
            name: String,
        }
        let name = |json| serde_json::from_str::<Named>(json).unwrap().name;
        assert_eq!((name(r#"{"name":"Sword"}"#), name(r#"{"name":null}"#), name(r#"{"name":7}"#)), ("Sword".into(), "".into(), "7".into()));
    }
}
