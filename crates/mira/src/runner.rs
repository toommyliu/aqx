use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;

use mira_client::{DeviceId, Event, Handle, Options, Phase, Reconnect, Server, api};
use mira_script::skua::SkuaScript;
use mira_script::{Cancel, LoadError, Outcome, Output, Script, Start};
use serde_json::{Map, Value};

pub struct Login {
    pub device: DeviceId,
    pub user: String,
    pub pass: String,
    pub client_version: Option<String>,
    pub relogin: Option<crate::config::Relogin>,
    pub api: Option<String>,
    pub record: Option<crate::record::Recorder>,
}

impl Login {
    pub fn spawn(
        &self,
        server: &str,
        user: &str,
        pass: &str,
    ) -> anyhow::Result<(Handle, Receiver<Event>)> {
        let account = match &self.api {
            Some(host) => api::login_on(host, &self.device, user, pass)?,
            None => api::login(&self.device, user, pass)?,
        };
        let server = account.pick_server(server)?.clone();
        let options = Options {
            client_version: self.client_version.clone(),
            trace_packets: self.record.is_some(),
            reconnect: self.relogin.map(|r| Reconnect {
                password: pass.to_owned(),
                delay: r.delay,
                attempts: r.attempts,
            }),
        };
        Ok(mira_client::spawn(&account, server, options)?)
    }

    pub fn connect(&self, server: &str) -> anyhow::Result<Handle> {
        self.connect_as(server, &self.user, &self.pass)
    }

    fn connect_as(&self, server: &str, user: &str, pass: &str) -> anyhow::Result<Handle> {
        let (session, events) = self.spawn(server, user, pass)?;
        match &self.record {
            Some(recorder) => recorder.follow(events),
            None => drop(events),
        }
        session.ready().wait()?;
        Ok(session)
    }
}

impl mira_script::Account for Login {
    fn servers(&self) -> Result<Vec<Server>, String> {
        match &self.api {
            Some(host) => api::servers_on(host, &self.device),
            None => api::servers(&self.device),
        }
        .map_err(|e| e.to_string())
    }

    fn login(&self, server: &str) -> Result<Handle, String> {
        self.connect(server).map_err(|e| format!("{e:#}"))
    }

    fn login_as(&self, server: &str, username: &str, password: &str) -> Result<Handle, String> {
        self.connect_as(server, username, password)
            .map_err(|e| format!("{e:#}"))
    }
}

pub enum Loaded {
    JavaScript(Script),
    Skua(SkuaScript),
}

impl Loaded {
    pub fn parse_args(&self, given: &[(String, String)]) -> Result<Map<String, Value>, String> {
        match self {
            Self::JavaScript(script) => script.parse_args(given),
            Self::Skua(script) => script.parse_args(given),
        }
    }

    fn run(self, start: Start) -> mira_script::Ended {
        match self {
            Self::JavaScript(script) => script.run(start),
            Self::Skua(script) => script.run(start),
        }
    }
}

pub struct Job {
    path: PathBuf,
    cancel: Cancel,
    output: Rc<dyn Fn(Output)>,
}

impl Job {
    pub fn new(path: &Path, cancel: &Cancel, output: impl Fn(Output) + 'static) -> Job {
        Job {
            path: path.to_path_buf(),
            cancel: cancel.clone(),
            output: Rc::new(output),
        }
    }

    pub fn is_skua(&self) -> bool {
        self.path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cs"))
    }

    pub fn load(&self) -> Result<Loaded, LoadError> {
        let output = Rc::clone(&self.output);
        let output = move |out| output(out);
        if self.is_skua() {
            let skua = crate::config::skua_home().map_err(LoadError::new)?;
            SkuaScript::load(&self.path, &skua, output, &self.cancel).map(Loaded::Skua)
        } else {
            Script::load(&self.path, output, &self.cancel).map(Loaded::JavaScript)
        }
    }
}

pub enum Note<'a> {
    Restarting,
    Reconnecting(&'a str),
    Session(&'a Handle),
}

pub struct Ran {
    pub outcome: Outcome,
    pub session: Handle,
    pub server: String,
}

pub fn drive(
    job: &Job,
    mut script: Loaded,
    args: &Map<String, Value>,
    account: &Arc<dyn mira_script::Account>,
    mut session: Handle,
    mut server: String,
    mut note: impl FnMut(Note),
) -> Ran {
    loop {
        let ended = script.run(Start {
            session,
            server: server.clone(),
            args: args.clone(),
            account: Arc::clone(account),
        });
        (session, server) = (ended.session, ended.server);
        note(Note::Session(&session));
        match ended.outcome {
            Outcome::Restart => note(Note::Restarting),
            Outcome::Disconnected(why) if reconnecting(&session) => {
                note(Note::Reconnecting(&why));
                match back_online(&session, &job.cancel) {
                    Some(Phase::Ready) => note(Note::Restarting),
                    Some(Phase::Closed { reason }) => {
                        return Ran {
                            outcome: Outcome::Disconnected(reason.to_string()),
                            session,
                            server,
                        };
                    }
                    _ => {
                        return Ran {
                            outcome: Outcome::Cancelled,
                            session,
                            server,
                        };
                    }
                }
            }
            outcome => {
                return Ran {
                    outcome,
                    session,
                    server,
                };
            }
        }
        script = match job.load() {
            Ok(script) => script,
            Err(_) if job.cancel.is_cancelled() => {
                return Ran {
                    outcome: Outcome::Cancelled,
                    session,
                    server,
                };
            }
            Err(e) => {
                return Ran {
                    outcome: Outcome::Failed(e.to_string()),
                    session,
                    server,
                };
            }
        };
    }
}

fn reconnecting(session: &Handle) -> bool {
    matches!(session.snapshot().phase, Phase::Reconnecting { .. })
}

fn back_online(session: &Handle, cancel: &Cancel) -> Option<Phase> {
    while !cancel.is_cancelled() {
        match &session.snapshot().phase {
            phase @ (Phase::Ready | Phase::Closed { .. }) => return Some(phase.clone()),
            _ => thread::sleep(Duration::from_millis(100)),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use mira_client::testing::{Harness, Peer, login_reply, serve_api};
    use mira_client::{DEFAULT_CLIENT_VERSION, Reconnect};
    use mira_script::{Account, Level};

    use super::*;

    struct Offline;

    impl Account for Offline {
        fn servers(&self) -> Result<Vec<Server>, String> {
            Err("no logins in this test".into())
        }

        fn login(&self, _: &str) -> Result<Handle, String> {
            Err("no logins in this test".into())
        }
    }

    #[test]
    fn a_reconnected_session_starts_the_script_again() {
        let (replies_in, replies) = mpsc::channel();
        let (stop_in, stop) = mpsc::channel();
        let (base, api) = serve_api(replies, stop);
        let (started_in, started) = mpsc::channel::<()>();
        let options = Options {
            reconnect: Some(Reconnect::new("pass1".into(), Duration::from_millis(100))),
            ..Options::default()
        };
        let harness = Harness::start_with_api(options, base, move |listener| {
            let port = listener.local_addr().unwrap().port();
            let mut peer = Peer::accept(&listener);
            peer.serve_to_ready();
            started.recv().unwrap();
            replies_in.send(login_reply("tok2", port)).unwrap();
            drop(peer);
            let mut peer = Peer::accept(&listener);
            peer.serve_to_ready_with_token(DEFAULT_CLIENT_VERSION, "tok2");
            peer.expect_logout();
        });
        harness.session.ready().wait().unwrap();
        let dir = std::env::temp_dir().join(format!("mira-runner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.ts");
        std::fs::write(
            &path,
            r#"import { script, wait } from "mira";
            export default function* () {
              script.log("started");
              yield* wait.ms(60_000);
            }"#,
        )
        .unwrap();
        let cancel = Cancel::default();
        let runs = Rc::new(std::cell::Cell::new(0));
        let job = Job::new(&path, &cancel, {
            let (cancel, runs) = (cancel.clone(), Rc::clone(&runs));
            move |out| {
                if let Output::Log {
                    level: Level::Log, ..
                } = out
                {
                    runs.set(runs.get() + 1);
                    match runs.get() {
                        1 => started_in.send(()).unwrap(),
                        _ => cancel.cancel(),
                    }
                }
            }
        });
        let script = job.load().unwrap();
        let account: Arc<dyn Account> = Arc::new(Offline);
        let mut notes = Vec::new();
        let ran = drive(
            &job,
            script,
            &Map::new(),
            &account,
            harness.session.clone(),
            "Test".into(),
            |note| match note {
                Note::Restarting => notes.push("restarting"),
                Note::Reconnecting(_) => notes.push("reconnecting"),
                Note::Session(_) => {}
            },
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(ran.outcome, Outcome::Cancelled);
        assert_eq!(runs.get(), 2, "the script ran once per session");
        assert_eq!(notes, ["reconnecting", "restarting"]);
        assert!(ran.session.same_session(&harness.session));
        harness.session.logout();
        harness.finish();
        stop_in.send(()).unwrap();
        assert_eq!(api.join().unwrap().len(), 1);
    }
}
