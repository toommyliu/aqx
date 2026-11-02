use std::io::Write;
use std::path::Path;

use aqx_protocol::{Decoder, Message, Xml};
use serde::Deserialize;
use serde_json::Value;

use crate::clock;

#[derive(Deserialize)]
struct Row {
    t: u64,
    dir: String,
    msg: String,
}

pub fn view(path: &Path, cmds: &[String], full: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let width = if full { usize::MAX } else { 160 };
    let mut decoder = Decoder::default();
    let mut out = std::io::stdout().lock();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Row>(line) else { continue };
        let arrow = match row.dir.as_str() {
            "out" => "→",
            "in" => "←",
            _ => "·",
        };
        let (name, detail) = match decoder.decode(row.msg.as_bytes()) {
            Ok(text) => describe(text),
            Err(e) => ("?".into(), format!("{e:?}")),
        };
        if !cmds.is_empty() && !cmds.contains(&name) {
            continue;
        }
        let line = format!("{}  {arrow}  {name:<18} {detail}", clock(row.t));
        let line = line.trim_end();
        let written = match line.char_indices().nth(width) {
            Some((cut, _)) => writeln!(out, "{}…", &line[..cut]),
            None => writeln!(out, "{line}"),
        };
        if written.is_err() {
            break;
        }
    }
    Ok(())
}

fn describe(text: &str) -> (String, String) {
    match Message::parse(text) {
        Ok(Message::Xml(xml)) => (xml.action.unwrap_or("xml").to_string(), xml_detail(&xml)),
        Ok(Message::Json(json)) => (json.cmd.to_string(), json_detail(json.body.get())),
        Ok(Message::Xt(xt)) if xt.cmd == "zm" => (xt.room.to_string(), xt.args().skip(1).collect::<Vec<_>>().join(" ")),
        Ok(Message::Xt(xt)) => (xt.cmd.to_string(), xt.args().collect::<Vec<_>>().join(" ")),
        Err(_) => ("?".into(), text.to_string()),
    }
}

fn xml_detail(xml: &Xml) -> String {
    match xml.action {
        Some("login") => between(xml.text, "<nick><![CDATA[", "]]>").unwrap_or_default().to_string(),
        Some("joinOK") => format!("{} players", xml.user_ids().count()),
        Some("uER" | "userGone") => xml.user_ids().map(|id| format!("entity {id}")).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

fn json_detail(body: &str) -> String {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else { return String::new() };
    fields
        .iter()
        .filter(|(k, _)| *k != "cmd")
        .map(|(k, v)| match v {
            Value::Array(a) => format!("{k}[{}]", a.len()),
            Value::Object(o) => format!("{k}{{{}}}", o.len()),
            Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    Some(&text[start..start + text[start..].find(close)?])
}
