use std::fmt::Write as _;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::http::Request;
use crate::config;
use crate::control::{Frame, Want};
use crate::daemon::{self, Ended, Session};
use crate::fleet::Stats;
use crate::view::{Shown, State};

const TICK: Duration = Duration::from_millis(250);
const HEARTBEAT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    None,
    Running {
        started_ms: u64,
        other_build: bool,
        config_changed: bool,
    },
    Incompatible {
        started_ms: u64,
    },
    Ending,
}

pub enum Seen {
    Idle(Vec<Shown>),
    Running {
        started_ms: u64,
        other_build: bool,
        config_changed: bool,
        frame: Frame,
    },
    Incompatible {
        started_ms: u64,
    },
}

pub struct Feed {
    kind: Option<Kind>,
    started_ms: Option<u64>,
    since: u64,
    sent: Vec<String>,
    ended: Option<SystemTime>,
}

impl Feed {
    pub fn resuming(last_event: Option<&str>) -> Feed {
        let resumed = last_event.and_then(|id| {
            let (started, seq) = id.split_once(':')?;
            Some((started.parse().ok()?, seq.parse::<u64>().ok()?))
        });
        Feed {
            kind: None,
            started_ms: resumed.map(|(started, _)| started),
            since: resumed.map_or(0, |(_, seq)| seq + 1),
            sent: Vec::new(),
            ended: None,
        }
    }

    pub fn since(&self) -> u64 {
        self.since
    }

    pub fn see(&mut self, seen: Seen) -> String {
        let mut out = String::new();
        let (kind, accounts, log) = match seen {
            Seen::Idle(accounts) => (Kind::None, accounts, Vec::new()),
            Seen::Incompatible { started_ms } => {
                (Kind::Incompatible { started_ms }, Vec::new(), Vec::new())
            }
            Seen::Running {
                started_ms,
                other_build,
                config_changed,
                frame,
            } => {
                let kind = if frame.ending {
                    Kind::Ending
                } else {
                    Kind::Running {
                        started_ms,
                        other_build,
                        config_changed,
                    }
                };
                (kind, frame.accounts, frame.log)
            }
        };
        if self.kind != Some(kind) {
            self.kind = Some(kind);
            let data = match kind {
                Kind::None => json!({ "kind": "none" }),
                Kind::Incompatible { started_ms } => {
                    json!({ "kind": "incompatible", "startedMs": started_ms })
                }
                Kind::Ending => json!({ "kind": "ending" }),
                Kind::Running {
                    started_ms,
                    other_build,
                    config_changed,
                } => json!({
                    "kind": "running",
                    "startedMs": started_ms,
                    "otherBuild": other_build,
                    "configChanged": config_changed,
                }),
            };
            event(&mut out, "session", None, &data.to_string());
        }
        let shown: Vec<String> = accounts
            .iter()
            .map(|a| serde_json::to_string(a).expect("accounts serialize"))
            .collect();
        if shown.len() != self.sent.len() {
            event(
                &mut out,
                "accounts",
                None,
                &format!("[{}]", shown.join(",")),
            );
        } else {
            for (i, (now, before)) in shown.iter().zip(&self.sent).enumerate() {
                if now != before {
                    event(
                        &mut out,
                        "account",
                        None,
                        &format!("{{\"i\":{i},\"account\":{now}}}"),
                    );
                }
            }
        }
        self.sent = shown;
        if let (Some(last), Some(started)) = (log.last(), self.started_ms) {
            self.since = last.seq + 1;
            let id = format!("{started}:{}", last.seq);
            let lines = serde_json::to_string(&log).expect("lines serialize");
            event(&mut out, "log", Some(&id), &lines);
        }
        out
    }

    pub fn ended(&mut self, ended: Option<&Ended>) -> String {
        let mut out = String::new();
        if let Some(ended) = ended
            && self.ended != Some(ended.at)
        {
            self.ended = Some(ended.at);
            let at_ms = ended
                .at
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let data = json!({ "endedMs": at_ms, "accounts": ended.accounts });
            event(&mut out, "ended", None, &data.to_string());
        }
        out
    }

    pub fn session_is(&mut self, started_ms: Option<u64>) -> bool {
        if self.started_ms == started_ms {
            return false;
        }
        self.started_ms = started_ms;
        self.since = 0;
        true
    }
}

fn event(out: &mut String, name: &str, id: Option<&str>, data: &str) {
    let _ = writeln!(out, "event: {name}");
    if let Some(id) = id {
        let _ = writeln!(out, "id: {id}");
    }
    let _ = write!(out, "data: {data}\n\n");
}

pub fn offline(account: &config::Account) -> Shown {
    Shown {
        name: account.name.clone(),
        user: account.user.clone(),
        state: State::Offline { why: None },
        server: String::new(),
        script: None,
        status: String::new(),
        up: None,
        stats: Stats::default(),
        gold_gained: None,
        game: None,
    }
}

struct Idle {
    read_at: Option<SystemTime>,
    accounts: Vec<Shown>,
    ended: Option<Ended>,
}

impl Idle {
    fn accounts(&mut self, config: &Path) -> Vec<Shown> {
        let modified = std::fs::metadata(config).and_then(|m| m.modified()).ok();
        if modified.is_none() || modified != self.read_at {
            self.read_at = modified;
            self.accounts = config::load_quietly(config)
                .map(|accounts| accounts.iter().map(offline).collect())
                .unwrap_or_default();
        }
        self.accounts.clone()
    }

    fn ended(&mut self, config: &Path) -> Option<&Ended> {
        let latest = daemon::last_session_at(config);
        if latest != self.ended.as_ref().map(|e| e.at) {
            self.ended = daemon::last_session(config);
        }
        self.ended.as_ref()
    }
}

fn changed_since(config: &Path, started_ms: u64) -> bool {
    std::fs::metadata(config)
        .and_then(|m| m.modified())
        .is_ok_and(|at| at > UNIX_EPOCH + Duration::from_millis(started_ms))
}

pub fn run(config: &Path, request: Request) {
    let selected = request
        .query("account")
        .and_then(|i| i.parse::<usize>().ok());
    let resume = request
        .last_event
        .clone()
        .or_else(|| request.query("resume"));
    let mut feed = Feed::resuming(resume.as_deref());
    let Some(mut out) = request.into_stream() else {
        return;
    };
    let mut session: Option<Session> = None;
    let mut idle = Idle {
        read_at: None,
        accounts: Vec::new(),
        ended: None,
    };
    let mut quiet_since = Instant::now();
    if out
        .write_all(b"retry: 1000\n\n")
        .and_then(|()| out.flush())
        .is_err()
    {
        return;
    }
    loop {
        if session.is_none() {
            session = daemon::session_for(config).ok().flatten();
        }
        let seen = match &session {
            None => {
                feed.session_is(None);
                Seen::Idle(idle.accounts(config))
            }
            Some(s) if !s.compatible() => {
                let started_ms = s.started_ms;
                feed.session_is(Some(started_ms));
                session = None;
                Seen::Incompatible { started_ms }
            }
            Some(s) => {
                feed.session_is(Some(s.started_ms));
                let want = Want {
                    selected: selected.unwrap_or(usize::MAX),
                    since: feed.since(),
                    full: selected.is_some(),
                };
                match s.source().frame(&want) {
                    Ok(frame) => Seen::Running {
                        started_ms: s.started_ms,
                        other_build: !s.same_build(),
                        config_changed: changed_since(config, s.started_ms),
                        frame,
                    },
                    Err(_) => {
                        session = None;
                        continue;
                    }
                }
            }
        };
        let idle_now = matches!(seen, Seen::Idle(_));
        let mut text = feed.see(seen);
        if idle_now {
            text.push_str(&feed.ended(idle.ended(config)));
        }
        let wrote = if !text.is_empty() {
            quiet_since = Instant::now();
            out.write_all(text.as_bytes()).and_then(|()| out.flush())
        } else if quiet_since.elapsed() >= HEARTBEAT {
            quiet_since = Instant::now();
            out.write_all(b": \n\n").and_then(|()| out.flush())
        } else {
            Ok(())
        };
        if wrote.is_err() {
            return;
        }
        thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::{Channel, Line};

    fn shown(name: &str, status: &str) -> Shown {
        Shown {
            status: status.into(),
            ..offline(
                &config::read(
                    &format!(
                        "[[account]]\nuser = \"{name}\"\npassword = \"x\"\nserver = \"Twilly\"\n"
                    ),
                    Path::new("/srv"),
                )
                .unwrap()[0],
            )
        }
    }

    fn line(seq: u64, text: &str) -> Line {
        Line {
            seq,
            at_ms: 0,
            channel: Channel::Mira,
            text: text.into(),
        }
    }

    fn running(accounts: Vec<Shown>, log: Vec<Line>) -> Seen {
        Seen::Running {
            started_ms: 7,
            other_build: false,
            config_changed: false,
            frame: Frame {
                accounts,
                log,
                ending: false,
            },
        }
    }

    fn events(text: &str) -> Vec<&str> {
        text.lines()
            .filter_map(|l| l.strip_prefix("event: "))
            .collect()
    }

    #[test]
    fn only_what_changed_is_sent_again() {
        let mut feed = Feed::resuming(None);
        feed.session_is(Some(7));
        let first = feed.see(running(
            vec![shown("player1", ""), shown("player2", "")],
            vec![line(0, "logged in")],
        ));
        assert_eq!(events(&first), ["session", "accounts", "log"]);
        assert!(first.contains("id: 7:0\n"));
        assert_eq!(feed.since(), 1);

        let same = feed.see(running(
            vec![shown("player1", ""), shown("player2", "")],
            vec![],
        ));
        assert_eq!(same, "", "nothing changed, so nothing is sent");

        let one = feed.see(running(
            vec![shown("player1", ""), shown("player2", "killing")],
            vec![],
        ));
        assert_eq!(events(&one), ["account"]);
        assert!(one.contains("\"i\":1"));
        assert!(!one.contains("player1"));
    }

    #[test]
    fn a_reconnect_resumes_the_log_after_its_last_line_in_the_same_session() {
        let mut feed = Feed::resuming(Some("7:41"));
        assert!(!feed.session_is(Some(7)));
        assert_eq!(feed.since(), 42);

        let mut feed = Feed::resuming(Some("7:41"));
        assert!(
            feed.session_is(Some(9)),
            "a new session starts its log over"
        );
        assert_eq!(feed.since(), 0);
    }

    #[test]
    fn the_session_state_is_sent_when_it_changes() {
        let mut feed = Feed::resuming(None);
        let idle = feed.see(Seen::Idle(vec![shown("player1", "")]));
        assert_eq!(events(&idle), ["session", "accounts"]);
        assert!(idle.contains("\"kind\":\"none\""));
        assert_eq!(feed.see(Seen::Idle(vec![shown("player1", "")])), "");
        feed.session_is(Some(7));
        let started = feed.see(running(vec![shown("player1", "")], vec![]));
        assert_eq!(events(&started), ["session"]);
        assert!(started.contains("\"startedMs\":7"));
        assert!(started.contains("\"configChanged\":false"));
        let edited = feed.see(Seen::Running {
            started_ms: 7,
            other_build: false,
            config_changed: true,
            frame: Frame {
                accounts: vec![shown("player1", "")],
                log: vec![],
                ending: false,
            },
        });
        assert_eq!(events(&edited), ["session"]);
        assert!(edited.contains("\"configChanged\":true"));
    }

    #[test]
    fn the_last_sessions_lines_are_sent_once_per_session() {
        let mut feed = Feed::resuming(None);
        assert_eq!(feed.ended(None), "");
        let ended = Ended {
            at: UNIX_EPOCH + Duration::from_secs(60),
            accounts: vec![crate::fleet::LastLines {
                name: "player1".into(),
                user: "player1".into(),
                lines: vec![line(3, "couldn't log in")],
            }],
        };
        let first = feed.ended(Some(&ended));
        assert_eq!(events(&first), ["ended"]);
        assert!(first.contains("\"endedMs\":60000"));
        assert!(first.contains("couldn't log in"));
        assert_eq!(
            feed.ended(Some(&ended)),
            "",
            "the same session isn't sent twice"
        );
        let later = Ended {
            at: UNIX_EPOCH + Duration::from_secs(90),
            ..ended
        };
        assert_eq!(events(&feed.ended(Some(&later))), ["ended"]);
    }
}
