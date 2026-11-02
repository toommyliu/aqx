use std::collections::BTreeMap;

use aqx_protocol::{Decoder, Framer, Message};
use serde::Deserialize;

#[derive(Deserialize)]
struct Row {
    dir: String,
    msg: String,
}

fn main() {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0;
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<Row> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        for dir in ["in", "out"] {
            let sent: Vec<&str> = rows.iter().filter(|r| r.dir == dir).map(|r| r.msg.as_str()).collect();
            let stream: Vec<u8> = sent.iter().flat_map(|m| m.bytes().chain([0])).collect();
            let mut framer = Framer::default();
            let mut decoder = Decoder::default();
            let mut seen = 0;
            let mut sizes = [1, 7, 64, 1500, 4096, 65536].iter().cycle();
            let mut rest = &stream[..];
            while !rest.is_empty() {
                let (chunk, tail) = rest.split_at((*sizes.next().unwrap()).min(rest.len()));
                rest = tail;
                framer
                    .feed(chunk, |frame| {
                        if frame != sent[seen].as_bytes() {
                            failures.push(format!("{path} {dir} #{seen}: frame differs from the recorded message"));
                        }
                        seen += 1;
                        let key = match decoder.decode(frame).map_err(|e| format!("{e:?}")).and_then(|t| {
                            Message::parse(t).map_err(|e| format!("{e:?}")).map(|m| match m {
                                Message::Xml(x) => format!("{dir} xml {}", x.action.unwrap_or("-")),
                                Message::Json(j) => format!("{dir} json {}", j.cmd),
                                Message::Xt(x) => format!("{dir} xt {}", x.cmd),
                            })
                        }) {
                            Ok(key) => key,
                            Err(e) => {
                                failures.push(format!("{path} {dir} #{seen}: {e}: {:.120}", String::from_utf8_lossy(frame)));
                                format!("{dir} error")
                            }
                        };
                        *counts.entry(key).or_default() += 1;
                    })
                    .unwrap();
            }
            if seen != sent.len() {
                failures.push(format!("{path} {dir}: {} messages recorded, {seen} framed", sent.len()));
            }
            total += seen;
        }
    }
    for (key, n) in &counts {
        println!("{n:6} {key}");
    }
    println!("{total} messages, {} failures", failures.len());
    for f in failures.iter().take(20) {
        println!("  {f}");
    }
    std::process::exit(if failures.is_empty() { 0 } else { 1 });
}
