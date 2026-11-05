use serde::de::Error;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Scalar {
    Int(i64),
    Bool(bool),
    Str(String),
}

pub fn int<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i64>,
{
    let n: i64 = match Scalar::deserialize(d)? {
        Scalar::Int(n) => n,
        Scalar::Str(s) => s
            .parse()
            .map_err(|_| D::Error::custom(format!("expected an integer, got {s:?}")))?,
        Scalar::Bool(b) => return Err(D::Error::custom(format!("expected an integer, got {b}"))),
    };
    T::try_from(n).map_err(|_| D::Error::custom(format!("{n} is out of range")))
}

pub fn bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    match Scalar::deserialize(d)? {
        Scalar::Bool(b) => Ok(b),
        Scalar::Int(0) => Ok(false),
        Scalar::Int(1) => Ok(true),
        Scalar::Str(s) if s == "0" || s == "false" => Ok(false),
        Scalar::Str(s) if s == "1" || s == "true" => Ok(true),
        _ => Err(D::Error::custom("expected a flag")),
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize, Debug, PartialEq)]
    struct Fields {
        #[serde(deserialize_with = "super::int")]
        port: u16,
        #[serde(deserialize_with = "super::bool")]
        online: bool,
    }

    fn parse(value: serde_json::Value) -> Result<Fields, serde_json::Error> {
        serde_json::from_value(value)
    }

    #[test]
    fn numbers_and_flags_parse_from_either_encoding() {
        let expected = Fields {
            port: 5588,
            online: true,
        };
        assert_eq!(parse(json!({"port": 5588, "online": 1})).unwrap(), expected);
        assert_eq!(
            parse(json!({"port": "5588", "online": true})).unwrap(),
            expected
        );
        assert_eq!(
            parse(json!({"port": 5588, "online": "1"})).unwrap(),
            expected
        );
        assert!(!parse(json!({"port": 1, "online": "false"})).unwrap().online);
    }

    #[test]
    fn values_outside_the_target_type_are_errors() {
        assert!(parse(json!({"port": 70000, "online": 1})).is_err());
        assert!(parse(json!({"port": "abc", "online": 1})).is_err());
        assert!(parse(json!({"port": 1, "online": 2})).is_err());
    }
}
