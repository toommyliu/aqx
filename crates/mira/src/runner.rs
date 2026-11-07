use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use mira_client::{DeviceId, Event, Handle, Options, Reconnect, Server, api};
use mira_script::skua::SkuaScript;
use mira_script::{Cancel, LoadError, Outcome, Output, Script, Start};
use serde_json::{Map, Value};

pub struct Login {
    pub device: DeviceId,
    pub user: String,
    pub pass: String,
    pub client_version: Option<String>,
    pub reconnect: Option<crate::config::Reconnect>,
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
            reconnect: self.reconnect.map(|r| Reconnect {
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
            SkuaScript::load(&self.path, output, &self.cancel).map(Loaded::Skua)
        } else {
            Script::load(&self.path, output, &self.cancel).map(Loaded::JavaScript)
        }
    }
}

pub enum Note<'a> {
    Restarting,
    Relogging {
        server: &'a str,
        delay: Duration,
        attempt: u32,
        attempts: u32,
    },
    ReloginFailed(String),
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
            Outcome::Relogin(policy) => {
                let target = policy.server.clone().unwrap_or_else(|| server.clone());
                let mut delay = policy.delay;
                let relogged = (1..=policy.attempts).find_map(|attempt| {
                    note(Note::Relogging {
                        server: &target,
                        delay,
                        attempt,
                        attempts: policy.attempts,
                    });
                    if !wait(delay, &job.cancel) {
                        return Some(None);
                    }
                    delay = delay.saturating_mul(2);
                    match account.login(&target) {
                        Ok(session) => Some(Some(session)),
                        Err(e) => {
                            note(Note::ReloginFailed(e));
                            None
                        }
                    }
                });
                let Some(Some(relogged)) = relogged else {
                    let outcome = if job.cancel.is_cancelled() {
                        Outcome::Cancelled
                    } else {
                        Outcome::Disconnected("couldn't log back in".into())
                    };
                    return Ran {
                        outcome,
                        session,
                        server,
                    };
                };
                note(Note::Session(&relogged));
                (session, server) = (relogged, target);
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

fn wait(delay: Duration, cancel: &Cancel) -> bool {
    let until = Instant::now() + delay;
    while Instant::now() < until {
        if cancel.is_cancelled() {
            return false;
        }
        thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
    }
    !cancel.is_cancelled()
}
