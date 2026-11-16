use serde::Deserialize;
use serde_json::{Map, Value, json};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike};

const SECRET: &str = "password";
const SCHEDULE: &str = "schedule";

#[derive(Deserialize)]
pub struct Form {
    #[serde(default)]
    defaults: Map<String, Value>,
    accounts: Vec<FormAccount>,
}

#[derive(Deserialize)]
pub struct FormAccount {
    from: Option<usize>,
    fields: Map<String, Value>,
    password: Option<String>,
}

pub struct Keys {
    settings: Vec<String>,
    account: Vec<String>,
    schedule: Vec<String>,
}

impl Keys {
    pub fn from_schema(schema: &Value) -> Keys {
        let keys = |def: &str| {
            schema["$defs"][def]["properties"]
                .as_object()
                .map(|p| {
                    p.keys()
                        .filter(|k| *k != SECRET && *k != SCHEDULE)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        Keys {
            settings: keys("Settings"),
            account: keys("AccountConfig"),
            schedule: keys("ScheduleConfig"),
        }
    }
}

pub fn read(text: &str) -> Result<Value, String> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    let defaults = doc
        .get("defaults")
        .and_then(Item::as_table_like)
        .map(fields_of)
        .unwrap_or_default();
    let accounts: Vec<Value> = doc
        .get("account")
        .and_then(Item::as_array_of_tables)
        .map(|tables| {
            tables
                .iter()
                .map(|t| json!({ "fields": fields_of(t), "hasPassword": t.contains_key(SECRET) }))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({ "defaults": defaults, "accounts": accounts }))
}

pub fn apply(text: &str, form: Form, keys: &Keys) -> Result<String, String> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    if !form.defaults.is_empty() || doc.contains_key("defaults") {
        let defaults = doc
            .entry("defaults")
            .or_insert(Item::Table(Table::new()))
            .as_table_like_mut()
            .ok_or("[defaults] isn't a table")?;
        set_fields(defaults, &form.defaults, &keys.settings)?;
    }
    let old: Vec<Table> = doc
        .get("account")
        .and_then(Item::as_array_of_tables)
        .map(|tables| tables.iter().cloned().collect())
        .unwrap_or_default();
    let mut accounts = ArrayOfTables::new();
    for account in form.accounts {
        let mut table = match account.from {
            Some(i) => old
                .get(i)
                .cloned()
                .ok_or("an account moved while you edited")?,
            None => Table::new(),
        };
        let mut fields = account.fields;
        let schedule = fields.remove(SCHEDULE);
        set_fields(&mut table, &fields, &keys.account)?;
        set_schedule(&mut table, schedule, &keys.schedule)?;
        match account.password.filter(|p| !p.is_empty()) {
            Some(password) => set(&mut table, SECRET, toml_edit::Value::from(password)),
            None if account.from.is_none() => return Err("a new account needs a password".into()),
            None => {}
        }
        accounts.push(table);
    }
    doc.insert("account", Item::ArrayOfTables(accounts));
    Ok(doc.to_string())
}

fn fields_of(table: &dyn TableLike) -> Map<String, Value> {
    entries_of(table).filter(|(key, _)| key != SECRET).collect()
}

fn entries_of(table: &dyn TableLike) -> impl Iterator<Item = (String, Value)> + '_ {
    table
        .iter()
        .filter_map(|(key, item)| Some((key.to_owned(), json_of_item(item)?)))
}

fn json_of_item(item: &Item) -> Option<Value> {
    match item {
        Item::Value(value) => Some(json_of(value)),
        Item::Table(table) => Some(entries_of(table).collect::<Map<_, _>>().into()),
        Item::ArrayOfTables(tables) => Some(
            tables
                .iter()
                .map(|t| Value::from(entries_of(t).collect::<Map<_, _>>()))
                .collect(),
        ),
        Item::None => None,
    }
}

fn json_of(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(s) => json!(s.value()),
        toml_edit::Value::Integer(i) => json!(i.value()),
        toml_edit::Value::Float(f) => json!(f.value()),
        toml_edit::Value::Boolean(b) => json!(b.value()),
        toml_edit::Value::Datetime(d) => json!(d.value().to_string()),
        toml_edit::Value::Array(a) => a.iter().map(json_of).collect(),
        toml_edit::Value::InlineTable(t) => t
            .iter()
            .map(|(k, v)| (k.to_owned(), json_of(v)))
            .collect::<Map<_, _>>()
            .into(),
    }
}

fn toml_of(value: &Value) -> Result<toml_edit::Value, String> {
    Ok(match value {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64().ok_or("a number is out of range")?.into(),
        },
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => items
            .iter()
            .map(toml_of)
            .collect::<Result<toml_edit::Array, _>>()?
            .into(),
        Value::Object(fields) => {
            let mut table = InlineTable::new();
            for (key, value) in fields {
                table.insert(key, toml_of(value)?);
            }
            table.into()
        }
        Value::Null => return Err("null isn't a TOML value".into()),
    })
}

fn set_fields(
    table: &mut dyn TableLike,
    fields: &Map<String, Value>,
    known: &[String],
) -> Result<(), String> {
    if let Some(unknown) = fields.keys().find(|key| !known.contains(key)) {
        return Err(format!("{unknown} isn't a setting"));
    }
    for key in known {
        match fields.get(key).filter(|v| !v.is_null()) {
            None => {
                table.remove(key);
            }
            Some(wanted) => {
                let same = table
                    .get(key)
                    .and_then(json_of_item)
                    .is_some_and(|now| now == *wanted);
                if !same {
                    set(table, key, toml_of(wanted)?);
                }
            }
        }
    }
    Ok(())
}

fn set_schedule(table: &mut Table, wanted: Option<Value>, known: &[String]) -> Result<(), String> {
    let wanted = match wanted {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => return Err("schedule must be a list".into()),
    };
    if wanted.is_empty() {
        table.remove(SCHEDULE);
        return Ok(());
    }
    let now = table.get(SCHEDULE).and_then(json_of_item);
    if now.as_ref().and_then(Value::as_array) == Some(&wanted) {
        return Ok(());
    }
    let old: Vec<Table> = table
        .get(SCHEDULE)
        .and_then(Item::as_array_of_tables)
        .map(|tables| tables.iter().cloned().collect())
        .unwrap_or_default();
    let mut tables = ArrayOfTables::new();
    for (i, entry) in wanted.iter().enumerate() {
        let fields = entry.as_object().ok_or("each schedule must be a table")?;
        let mut entry = old.get(i).cloned().unwrap_or_default();
        set_fields(&mut entry, fields, known)?;
        tables.push(entry);
    }
    table.insert(SCHEDULE, Item::ArrayOfTables(tables));
    Ok(())
}

fn set(table: &mut dyn TableLike, key: &str, mut value: toml_edit::Value) {
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(old) => {
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        None => {
            table.insert(key, Item::Value(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"# my accounts
[defaults]
server = "Twilly" # most of them

# the farmer
[[account]]
user = "player1"
password = "secret-one"
script = "farm.ts"
args = { target = 7500 }

# after the reset
[[account.schedule]]
at = "00:05"
tz = "America/New_York"
script = "dailies.ts"

[[account.schedule]]
at = "9pm"
days = ["fri"]
script = "weekly.ts"
args = { fast = true }

[[account]]
name = "healer"
user = "player2"
password = "secret-two"

[account.args]
leader = "player1"
"#;

    fn keys() -> Keys {
        Keys::from_schema(&serde_json::from_str(&crate::config::schema()).unwrap())
    }

    fn form(value: Value) -> Form {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_form_never_carries_a_password() {
        let read = read(FILE).unwrap();
        let text = read.to_string();
        assert!(!text.contains("secret"), "{text}");
        assert_eq!(read["accounts"][0]["hasPassword"], true);
        assert_eq!(read["accounts"][0]["fields"]["args"]["target"], 7500);
        assert_eq!(read["accounts"][1]["fields"]["args"]["leader"], "player1");
        assert_eq!(read["defaults"]["server"], "Twilly");
        assert_eq!(
            read["accounts"][0]["fields"]["schedule"][1]["days"][0],
            "fri"
        );
        assert_eq!(
            read["accounts"][0]["fields"]["schedule"][1]["args"]["fast"],
            true
        );
    }

    #[test]
    fn a_schedule_edit_keeps_the_other_schedules_as_written() {
        let read = read(FILE).unwrap();
        let mut farmer = read["accounts"][0]["fields"].clone();
        farmer["schedule"][1]["at"] = json!("10pm");
        let schedules = farmer["schedule"].as_array().unwrap().clone();
        let saved = apply(
            FILE,
            form(json!({ "defaults": read["defaults"], "accounts": [{ "from": 0, "fields": farmer, "password": null }] })),
            &keys(),
        )
        .unwrap();
        assert!(
            saved.contains("# after the reset\n[[account.schedule]]\nat = \"00:05\""),
            "{saved}"
        );
        assert!(saved.contains("at = \"10pm\"\ndays = [\"fri\"]"), "{saved}");
        let farmer = json!({ "user": "player1", "script": "farm.ts", "schedule": [schedules[0]] });
        let saved = apply(
            FILE,
            form(json!({ "defaults": read["defaults"], "accounts": [{ "from": 0, "fields": farmer, "password": null }] })),
            &keys(),
        )
        .unwrap();
        assert!(
            !saved.contains("weekly.ts") && saved.contains("dailies.ts"),
            "{saved}"
        );
        let farmer = json!({ "user": "player1" });
        let saved = apply(
            FILE,
            form(json!({ "defaults": read["defaults"], "accounts": [{ "from": 0, "fields": farmer, "password": null }] })),
            &keys(),
        )
        .unwrap();
        assert!(!saved.contains("schedule"), "{saved}");
        let accounts = crate::config::read(&saved, std::path::Path::new("/srv")).unwrap();
        assert!(accounts[0].schedules.is_empty());
    }

    #[test]
    fn saving_the_form_unchanged_leaves_the_file_as_it_was() {
        let read = read(FILE).unwrap();
        let accounts: Vec<Value> = read["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, a)| json!({ "from": i, "fields": a["fields"], "password": null }))
            .collect();
        let saved = apply(
            FILE,
            form(json!({ "defaults": read["defaults"], "accounts": accounts })),
            &keys(),
        )
        .unwrap();
        assert_eq!(saved, FILE);
    }

    #[test]
    fn an_edit_changes_only_its_own_line_and_keeps_passwords_and_comments() {
        let saved = apply(
            FILE,
            form(json!({
                "defaults": { "server": "Yorumi" },
                "accounts": [
                    { "from": 1, "fields": { "name": "healer", "user": "player2", "auto_start": false }, "password": null },
                    { "from": null, "fields": { "user": "player3" }, "password": "secret-three" },
                ],
            })),
            &keys(),
        )
        .unwrap();
        assert!(
            saved.starts_with("# my accounts\n[defaults]\nserver = \"Yorumi\" # most of them\n"),
            "{saved}"
        );
        assert!(
            !saved.contains("player1"),
            "the dropped account is gone: {saved}"
        );
        assert!(saved.contains("password = \"secret-two\""), "{saved}");
        assert!(saved.contains("auto_start = false"), "{saved}");
        assert!(
            saved.contains("user = \"player3\"\npassword = \"secret-three\""),
            "{saved}"
        );
        let accounts = crate::config::read(&saved, std::path::Path::new("/srv")).unwrap();
        let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["healer", "player3"]);
    }

    #[test]
    fn the_form_refuses_what_aqx_toml_cannot_hold() {
        let unknown = apply(
            FILE,
            form(
                json!({ "accounts": [{ "from": 0, "fields": { "user": "player1", "colour": "red" }, "password": null }] }),
            ),
            &keys(),
        );
        assert!(unknown.unwrap_err().contains("colour"));
        let no_password = apply(
            FILE,
            form(
                json!({ "accounts": [{ "from": null, "fields": { "user": "player9" }, "password": null }] }),
            ),
            &keys(),
        );
        assert!(no_password.unwrap_err().contains("password"));
    }
}
