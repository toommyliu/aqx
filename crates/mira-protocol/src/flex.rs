use serde::de::Error;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Scalar {
    Int(i64),
    Float(f64),
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
        Scalar::Float(f) if f.fract() == 0.0 && (i64::MIN as f64..i64::MAX as f64).contains(&f) => {
            f as i64
        }
        Scalar::Float(f) => return Err(D::Error::custom(format!("expected an integer, got {f}"))),
        Scalar::Str(s) => s
            .parse()
            .map_err(|_| D::Error::custom(format!("expected an integer, got {s:?}")))?,
        Scalar::Bool(b) => return Err(D::Error::custom(format!("expected an integer, got {b}"))),
    };
    T::try_from(n).map_err(|_| D::Error::custom(format!("{n} is out of range")))
}

pub fn float<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    match Scalar::deserialize(d)? {
        Scalar::Int(n) => Ok(n as f64),
        Scalar::Float(f) => Ok(f),
        Scalar::Str(s) => s
            .parse()
            .map_err(|_| D::Error::custom(format!("expected a number, got {s:?}"))),
        Scalar::Bool(b) => Err(D::Error::custom(format!("expected a number, got {b}"))),
    }
}

pub fn opt_int<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: TryFrom<i64>,
{
    int(d).map(Some)
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
        assert_eq!(
            parse(serde_json::from_str(r#"{"port": 5.588E3, "online": 1}"#).unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn floats_parse_from_any_numeric_encoding() {
        #[derive(Deserialize)]
        struct Rate {
            #[serde(deserialize_with = "super::float")]
            rate: f64,
        }
        let rate = |value| serde_json::from_value::<Rate>(value).map(|r| r.rate).ok();
        assert_eq!(rate(json!({"rate": 50})), Some(50.0));
        assert_eq!(rate(json!({"rate": 12.5})), Some(12.5));
        assert_eq!(rate(json!({"rate": "7.5"})), Some(7.5));
        assert_eq!(rate(json!({"rate": "abc"})), None);
    }

    #[test]
    fn values_outside_the_target_type_are_errors() {
        assert!(parse(json!({"port": 70000, "online": 1})).is_err());
        assert!(parse(json!({"port": "abc", "online": 1})).is_err());
        assert!(parse(json!({"port": 5588.5, "online": 1})).is_err());
        assert!(parse(json!({"port": 1, "online": 2})).is_err());
    }
}
