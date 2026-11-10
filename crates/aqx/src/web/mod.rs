mod feed;
mod form;
mod http;
mod scripts;

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use anyhow::Context;
use aqx_script::skua::library::{self, Updated};
use aqx_script::skua::{self as skua_options, Kind, Settings};
use aqx_script::{Arg, ArgKind, Scalar};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{self, Script};
use crate::control::{Action, Want};
use crate::daemon::{self, Session, Wanted};
use crate::view::{Shown, State};
use http::{Failure, Reply, Request};
use scripts::{History, Recent, list};

pub const PORT: u16 = 4970;

type Pair = (String, String);

type Remembered = HashMap<(PathBuf, String, Option<String>), String>;

pub struct Web {
    config: PathBuf,
    home: PathBuf,
    starting: Mutex<()>,
    saving: Mutex<()>,
    runs: Mutex<Runs>,
    updating: Mutex<()>,
}

struct Runs {
    history: History,
    recent: Recent,
    remembered: Remembered,
    hidden: BTreeSet<PathBuf>,
}

impl Runs {
    fn save_hidden(&self) -> Result<(), Failure> {
        let path = hidden_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(Failure::internal)?;
        }
        let text = serde_json::to_string_pretty(&self.hidden).expect("paths serialize");
        write_private(&path, &text).map_err(Failure::internal)
    }
}

fn hidden_path() -> Result<PathBuf, Failure> {
    dirs::data_dir()
        .map(|dir| dir.join("aqx").join("web-hidden-recent.json"))
        .ok_or_else(|| Failure::internal("no data folder for the recent scripts"))
}

fn load_hidden() -> BTreeSet<PathBuf> {
    hidden_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn serve(config: PathBuf, port: u16) -> anyhow::Result<()> {
    let home = config
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let history = History::in_data_dir();
    let mut recent = Recent::default();
    history.load(&mut recent);
    let web = Arc::new(Web {
        config,
        home,
        starting: Mutex::new(()),
        saving: Mutex::new(()),
        runs: Mutex::new(Runs {
            history,
            recent,
            remembered: Remembered::new(),
            hidden: load_hidden(),
        }),
        updating: Mutex::new(()),
    });
    let address = format!("127.0.0.1:{port}");
    let server = tiny_http::Server::http(&address)
        .map_err(|e| anyhow::anyhow!("couldn't listen on {address}: {e}"))?;
    if !http::built() {
        eprintln!(
            "aqx: the web UI isn't built into this aqx; run npm --prefix web ci && npm --prefix web run build, then build aqx again"
        );
    }
    println!(
        "aqx web is serving {} at http://{address}",
        web.config.display()
    );
    for request in server.incoming_requests() {
        let web = Arc::clone(&web);
        thread::spawn(move || web.answer(request));
    }
    Ok(())
}

impl Web {
    fn answer(&self, request: tiny_http::Request) {
        let request = match Request::read(request) {
            Ok(request) => request,
            Err(refused) => return refused.send(),
        };
        if request.path == "/api/events" {
            return feed::run(&self.config, request);
        }
        let reply = self.route(&request).unwrap_or_else(Failure::into_reply);
        request.respond(reply);
    }

    fn route(&self, r: &Request) -> Result<Reply, Failure> {
        let post = r.method == tiny_http::Method::Post;
        match (r.path.as_str(), post) {
            ("/api/act", true) => self.act(r.body()?),
            ("/api/run", true) => self.run(r.body()?),
            ("/api/args", false) => self.args(r),
            ("/api/recent", false) => Ok(self.recent()),
            ("/api/recent/forget", true) => self.forget(r.body()?),
            ("/api/servers", false) => self.servers(r),
            ("/api/servers/all", false) => all_servers(),
            ("/api/files", false) => self.files(r),
            ("/api/skua", false) => Ok(skua_library(r.query("q").unwrap_or_default())),
            ("/api/skua/update", true) => self.update_skua(),
            ("/api/skua/options", false) => self.skua_options(r),
            ("/api/skua/options", true) => self.save_skua_options(r.body()?),
            ("/api/config", false) => self.read_config(),
            ("/api/config", true) => self.write_config(r.body()?),
            ("/api/keys", false) => self.read_keys(),
            ("/api/keys", true) => self.write_keys(r.body()?),
            ("/api/session/end", true) => self.end(r.body()?),
            ("/api/session/restart", true) => self.restart(r.body()?),
            (path, false) if !path.starts_with("/api/") => Ok(http::asset(path)),
            _ => Err(Failure::not_found()),
        }
    }

    fn session(&self) -> Result<Option<Session>, Failure> {
        match daemon::session_for(&self.config).map_err(Failure::internal)? {
            Some(session) if !session.compatible() => Err(Failure::conflict(
                "the session was started by an incompatible build of aqx; restart it",
            )),
            found => Ok(found),
        }
    }

    fn roster(&self) -> Result<Vec<(String, String)>, Failure> {
        self.roster_of(self.session()?.as_ref())
    }

    fn roster_of(&self, session: Option<&Session>) -> Result<Vec<(String, String)>, Failure> {
        if let Some(session) = session {
            return Ok(shown(session)?
                .into_iter()
                .map(|a| (a.name, a.user))
                .collect());
        }
        let accounts = config::load_quietly(&self.config).map_err(Failure::internal)?;
        Ok(accounts.into_iter().map(|a| (a.name, a.user)).collect())
    }

    fn act(&self, asked: ActRequest) -> Result<Reply, Failure> {
        if asked.names.len() != asked.accounts.len() {
            return Err(Failure::bad("name each account"));
        }
        let actions = asked
            .accounts
            .into_iter()
            .zip(asked.names)
            .map(|(i, name)| (i, name, asked.action.clone()))
            .collect();
        self.perform(actions)
    }

    fn perform(&self, actions: Vec<(usize, String, Action)>) -> Result<Reply, Failure> {
        let _starting = self.starting.lock().unwrap_or_else(PoisonError::into_inner);
        let session = self.session()?;
        let roster = self.roster_of(session.as_ref())?;
        let moved = actions
            .iter()
            .any(|(i, name, _)| roster.get(*i).is_none_or(|(now, _)| now != name));
        if moved {
            return Err(Failure::conflict(
                "the account list changed; reload the page and try again",
            ));
        }
        let mut actions: Vec<(usize, Action)> = actions
            .into_iter()
            .map(|(i, _, action)| (i, action))
            .collect();
        let mut outcomes = Vec::new();
        let session = match session {
            Some(session) => session,
            None => {
                let starts =
                    |a: &Action| matches!(a, Action::Login | Action::Relogin | Action::Run(_));
                let Some(at) = actions.iter().position(|(_, a)| starts(a)) else {
                    return Err(Failure::conflict(
                        "aqx isn't running; log an account in first",
                    ));
                };
                let (first, action) = actions.remove(at);
                let name = roster
                    .get(first)
                    .map(|(name, _)| name.clone())
                    .ok_or_else(|| Failure::bad("no such account"))?;
                let script = match action {
                    Action::Run(script) => Some(script),
                    _ => None,
                };
                let wanted = Wanted {
                    accounts: vec![name],
                    script,
                };
                let session = daemon::start(&self.config, &wanted).map_err(Failure::internal)?;
                outcomes.push(Outcome {
                    account: first,
                    error: None,
                });
                session
            }
        };
        for (account, action) in actions {
            let error = session.source().act(account, action).err();
            outcomes.push(Outcome { account, error });
        }
        Ok(Reply::json(&outcomes))
    }

    fn script_path(&self, typed: &str) -> Result<PathBuf, Failure> {
        let path = resolve(&self.home, typed);
        if !path.is_file() {
            return Err(Failure::bad(format!("{} isn't a file", path.display())));
        }
        Ok(path)
    }

    fn args(&self, r: &Request) -> Result<Reply, Failure> {
        let path = self.script_path(&r.query("script").unwrap_or_default())?;
        let declared = declared(&path)?;
        let names = self.roster()?;
        let accounts: Vec<String> = r
            .indices("accounts")
            .into_iter()
            .filter_map(|i| names.get(i).map(|(name, _)| name.clone()))
            .collect();
        let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        let recall = |name: &str, account: Option<&str>| {
            runs.remembered
                .get(&(path.clone(), name.to_owned(), account.map(str::to_owned)))
                .cloned()
        };
        let args: Vec<Value> = declared
            .iter()
            .map(|arg| {
                let each: Vec<Option<String>> = accounts
                    .iter()
                    .map(|a| recall(&arg.name, Some(a)))
                    .collect();
                json!({
                    "name": arg.name,
                    "kind": arg_kind(arg),
                    "optional": arg.optional,
                    "default": arg.default,
                    "shared": recall(&arg.name, None),
                    "each": each,
                })
            })
            .collect();
        Ok(Reply::json(&json!({
            "path": path,
            "shown": scripts::shown_path(&self.home, &path),
            "args": args,
        })))
    }

    fn run(&self, asked: RunRequest) -> Result<Reply, Failure> {
        let path = self.script_path(&asked.script)?;
        let declared = declared(&path)?;
        let names = self.roster()?;
        let mut problems = Vec::new();
        let mut actions = Vec::new();
        let mut answers = Vec::new();
        for run in &asked.runs {
            let name = names.get(run.account).map(|(name, _)| name.clone());
            let (mut args, values) = split_args(&declared, &asked.extra, &run.args);
            for (arg, given) in values {
                answers.push((arg.name.clone(), name.clone(), given.to_owned()));
                if given.is_empty() && arg.optional {
                    continue;
                }
                match arg.parse(given) {
                    Ok(_) => args.push((arg.name.clone(), given.to_owned())),
                    Err(error) => problems.push(json!({
                        "account": run.account,
                        "arg": arg.name,
                        "error": error,
                    })),
                }
            }
            let script = Script {
                path: path.clone(),
                args,
            };
            actions.push((run.account, run.name.clone(), Action::Run(script)));
        }
        if !problems.is_empty() {
            return Ok(Reply::json(&json!({ "problems": problems })).status(422));
        }
        {
            let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
            let shared = asked.runs.len() > 1 && asked.shared;
            for (arg, account, value) in answers {
                let account = if shared { None } else { account };
                runs.remembered.insert((path.clone(), arg, account), value);
            }
            runs.recent.add(Script {
                path: path.clone(),
                args: asked.extra.clone(),
            });
            if runs.hidden.remove(&path) && runs.save_hidden().is_err() {
                eprintln!("aqx: couldn't save the hidden recent scripts");
            }
            let Runs {
                history, recent, ..
            } = &*runs;
            if let Err(e) = history.save(recent) {
                eprintln!("aqx: couldn't save the run history: {e}");
            }
        }
        self.perform(actions)
    }

    fn recent(&self) -> Reply {
        let runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        let shown: Vec<Value> = runs
            .recent
            .typed(Path::new("/"))
            .iter()
            .rev()
            .filter_map(|typed| scripts::parse(typed, Path::new("/")).ok())
            .map(|script| {
                let path = resolve(Path::new("/"), &script.path.to_string_lossy());
                json!({
                    "shown": scripts::shown_path(&self.home, &path),
                    "path": path,
                    "args": script.args,
                })
            })
            .collect();
        Reply::json(&json!({ "runs": shown, "hidden": runs.hidden }))
    }

    fn forget(&self, asked: ForgetRequest) -> Result<Reply, Failure> {
        let wanted = resolve(Path::new("/"), &asked.path);
        let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        runs.recent
            .forget(|path| resolve(Path::new("/"), &path.to_string_lossy()) == wanted);
        runs.hidden.insert(wanted);
        runs.save_hidden()?;
        let Runs {
            history, recent, ..
        } = &*runs;
        history.save(recent).map_err(Failure::internal)?;
        drop(runs);
        Ok(self.recent())
    }

    fn servers(&self, r: &Request) -> Result<Reply, Failure> {
        let session = self
            .session()?
            .ok_or_else(|| Failure::conflict("aqx isn't running"))?;
        let listed = session.source().servers(&r.indices("accounts"));
        match listed.recv() {
            Ok(Ok(servers)) => Ok(Reply::json(&servers)),
            Ok(Err(why)) => Err(Failure::conflict(why)),
            Err(_) => Err(Failure::internal("the server list didn't arrive")),
        }
    }

    fn files(&self, r: &Request) -> Result<Reply, Failure> {
        let typed = r.query("dir").unwrap_or_default();
        let scripts = self.home.join("scripts");
        let dir = if typed.is_empty() && scripts.is_dir() {
            scripts
        } else if typed.is_empty() {
            self.home.clone()
        } else {
            resolve(&self.home, &typed)
        };
        let entries = list(&dir).map_err(Failure::bad)?;
        let entries: Vec<Value> = entries
            .iter()
            .map(|e| json!({ "name": e.name.trim_end_matches('/'), "dir": e.name.ends_with('/') }))
            .collect();
        Ok(Reply::json(&json!({
            "dir": dir,
            "shown": match scripts::shown_path(&self.home, &dir) {
                shown if shown.is_empty() => dir.display().to_string(),
                shown => shown,
            },
            "parent": dir.parent(),
            "entries": entries,
        })))
    }

    fn update_skua(&self) -> Result<Reply, Failure> {
        let Ok(_updating) = self.updating.try_lock() else {
            return Err(Failure::conflict("the Skua scripts are already updating"));
        };
        let home = config::skua_home().map_err(Failure::internal)?;
        let updated = library::update(&home).map_err(Failure::internal)?;
        aqx_script::skua::worker(&mut || {}).map_err(Failure::internal)?;
        let message = match updated {
            Updated::Current => "the Skua scripts are up to date".to_owned(),
            Updated::To(commit) => {
                format!("updated the Skua scripts to {}", library::short(&commit))
            }
        };
        Ok(Reply::json(&json!({ "message": message })))
    }

    fn skua_options(&self, r: &Request) -> Result<Reply, Failure> {
        let script = self.script_path(&r.query("script").unwrap_or_default())?;
        let user = self.user_of(
            r.indices("account").first().copied(),
            &r.query("name").unwrap_or_default(),
        )?;
        let home = config::skua_home().map_err(Failure::internal)?;
        let core = {
            let (home, user) = (home.clone(), user.clone());
            thread::spawn(move || {
                skua_options::core_bots_options(&user, &home, &[]).map_err(|e| e.to_string())
            })
        };
        let own = skua_options::script_options(&script, &home, &[]).map_err(|e| e.to_string());
        let core = core
            .join()
            .unwrap_or_else(|_| Err("the Skua worker stopped".to_owned()));
        Ok(Reply::json(&json!({
            "corebots": pane(core),
            "script": pane(own),
        })))
    }

    fn save_skua_options(&self, asked: OptionsRequest) -> Result<Reply, Failure> {
        let user = self.user_of(Some(asked.account), &asked.name)?;
        if let Some(session) = self.session()?
            && shown(&session)?
                .get(asked.account)
                .is_some_and(|a| !editable(&a.state))
        {
            return Err(Failure::conflict(
                "Options can change while the account is offline or idle.",
            ));
        }
        let home = config::skua_home().map_err(Failure::internal)?;
        let saved = match asked.tab.as_str() {
            "corebots" => skua_options::core_bots_options(&user, &home, &asked.changes),
            "script" => {
                let script = self.script_path(&asked.script)?;
                skua_options::script_options(&script, &home, &asked.changes)
            }
            _ => return Err(Failure::bad("tab is corebots or script")),
        };
        saved
            .map(|settings| Reply::json(&pane(Ok(settings))))
            .map_err(|e| Failure::bad(e.to_string()))
    }

    fn user_of(&self, account: Option<usize>, name: &str) -> Result<String, Failure> {
        let names = self.roster()?;
        account
            .and_then(|i| names.get(i))
            .filter(|(now, _)| now == name)
            .map(|(_, user)| user.clone())
            .ok_or_else(|| {
                Failure::conflict("the account list changed; reload the page and try again")
            })
    }

    fn read_config(&self) -> Result<Reply, Failure> {
        let text = std::fs::read_to_string(&self.config).map_err(Failure::internal)?;
        let form = form::read(&text).map_err(Failure::bad)?;
        let schema: Value = serde_json::from_str(&config::schema()).expect("the schema is JSON");
        Ok(Reply::json(&json!({
            "path": self.config,
            "version": version(&text),
            "text": text,
            "form": form,
            "schema": schema["$defs"],
            "clientVersion": aqx_client::DEFAULT_CLIENT_VERSION,
        })))
    }

    fn write_config(&self, asked: ConfigRequest) -> Result<Reply, Failure> {
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        let current = std::fs::read_to_string(&self.config).map_err(Failure::internal)?;
        if asked.version != version(&current) {
            return Err(Failure::conflict(
                "aqx.toml changed since you opened it; reload it and make your change again",
            ));
        }
        let text = match asked.edit {
            Edit::Text(text) => text,
            Edit::Form(edited) => {
                let schema: Value =
                    serde_json::from_str(&config::schema()).expect("the schema is JSON");
                form::apply(&current, edited, &form::Keys::from_schema(&schema))
                    .map_err(Failure::bad)?
            }
        };
        config::read(&text, &self.home).map_err(Failure::bad)?;
        write_private(&self.config, &text).map_err(Failure::internal)?;
        self.read_config()
    }

    fn read_keys(&self) -> Result<Reply, Failure> {
        let saved = match std::fs::read_to_string(keys_path()?) {
            Ok(text) => serde_json::from_str(&text).map_err(Failure::internal)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(Failure::internal(e)),
        };
        Ok(Reply::json(&saved))
    }

    fn write_keys(&self, keys: HashMap<String, Option<KeyBinding>>) -> Result<Reply, Failure> {
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        let path = keys_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(Failure::internal)?;
        }
        let text = serde_json::to_string_pretty(&keys).expect("keys serialize");
        write_private(&path, &text).map_err(Failure::internal)?;
        Ok(Reply::json(&keys))
    }

    fn this_session(&self, started_ms: u64) -> Result<Session, Failure> {
        match daemon::session_for(&self.config).map_err(Failure::internal)? {
            Some(session) if session.started_ms == started_ms => Ok(session),
            Some(_) => Err(Failure::conflict(
                "the session changed since this page loaded; check it and try again",
            )),
            None => Err(Failure::conflict("aqx isn't running")),
        }
    }

    fn end(&self, asked: SessionRequest) -> Result<Reply, Failure> {
        let session = self.this_session(asked.started_ms)?;
        if session.compatible() {
            session.source().end().map_err(Failure::conflict)?;
        } else {
            session.down().map_err(Failure::internal)?;
        }
        Ok(Reply::json(&json!({ "ended": true })))
    }

    fn restart(&self, asked: SessionRequest) -> Result<Reply, Failure> {
        let _starting = self.starting.lock().unwrap_or_else(PoisonError::into_inner);
        let old = self.this_session(asked.started_ms)?;
        let resume = old.resumable().map_err(|e| {
            Failure::conflict(format!(
                "couldn't read which accounts are online, so nothing was restarted: {e}"
            ))
        })?;
        let configured: Vec<String> = config::load_quietly(&self.config)
            .map_err(Failure::bad)?
            .into_iter()
            .map(|a| a.name)
            .collect();
        let (resume, gone): (Vec<_>, Vec<_>) = resume
            .into_iter()
            .partition(|(name, _)| configured.contains(name));
        old.down().map_err(Failure::internal)?;
        let mut resume = resume.into_iter();
        let Some((first, script)) = resume.next() else {
            return Ok(Reply::json(
                &json!({ "restarted": false, "gone": gone.len() }),
            ));
        };
        let fresh = daemon::start(
            &self.config,
            &Wanted {
                accounts: vec![first],
                script,
            },
        )
        .map_err(Failure::internal)?;
        let names = self.roster_of(Some(&fresh))?;
        for (name, script) in resume {
            if let Some(i) = names.iter().position(|(n, _)| *n == name) {
                let _ = fresh
                    .source()
                    .act(i, script.map_or(Action::Login, Action::Run));
            }
        }
        Ok(Reply::json(&json!({ "restarted": true })))
    }
}

#[derive(Deserialize)]
struct ActRequest {
    accounts: Vec<usize>,
    names: Vec<String>,
    action: Action,
}

#[derive(Deserialize)]
struct RunRequest {
    script: String,
    #[serde(default)]
    extra: Vec<(String, String)>,
    runs: Vec<RunFor>,
    #[serde(default)]
    shared: bool,
}

#[derive(Deserialize)]
struct RunFor {
    account: usize,
    name: String,
    #[serde(default)]
    args: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRequest {
    started_ms: u64,
}

#[derive(Deserialize)]
struct OptionsRequest {
    account: usize,
    name: String,
    script: String,
    tab: String,
    changes: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct ForgetRequest {
    path: String,
}

#[derive(Deserialize)]
struct ConfigRequest {
    version: String,
    #[serde(flatten)]
    edit: Edit,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Edit {
    Text(String),
    Form(form::Form),
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum KeyBinding {
    Chord(String),
    Sequence(Vec<String>),
}

fn keys_path() -> Result<PathBuf, Failure> {
    dirs::data_dir()
        .map(|dir| dir.join("aqx").join("web-keys.json"))
        .ok_or_else(|| Failure::internal("no data folder for the keys"))
}

fn version(text: &str) -> String {
    use std::hash::{DefaultHasher, Hasher};
    let mut hasher = DefaultHasher::new();
    hasher.write(text.as_bytes());
    format!("{:016x}", hasher.finish())
}

#[derive(Serialize)]
struct Outcome {
    account: usize,
    error: Option<String>,
}

fn shown(session: &Session) -> Result<Vec<Shown>, Failure> {
    let frame = session
        .source()
        .frame(&Want {
            selected: usize::MAX,
            since: u64::MAX,
            full: false,
        })
        .map_err(Failure::internal)?;
    Ok(frame.accounts)
}

fn editable(state: &State) -> bool {
    matches!(state, State::Offline { .. } | State::Online)
}

fn all_servers() -> Result<Reply, Failure> {
    let device = aqx_client::DeviceId::load().map_err(Failure::internal)?;
    let servers = aqx_client::api::servers(&device).map_err(Failure::internal)?;
    let listed: Vec<Value> = servers
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "players": s.players,
                "capacity": s.capacity,
                "members": s.members_only,
                "online": s.online,
            })
        })
        .collect();
    Ok(Reply::json(&listed))
}

fn declared(path: &Path) -> Result<Vec<Arg>, Failure> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cs"))
    {
        return Ok(Vec::new());
    }
    aqx_script::script_args(path).map_err(Failure::bad)
}

fn split_args<'a>(
    declared: &'a [Arg],
    typed: &'a [Pair],
    fields: &'a [Pair],
) -> (Vec<Pair>, Vec<(&'a Arg, &'a str)>) {
    let value_in = |pairs: &'a [Pair], name: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    };
    let values = declared
        .iter()
        .map(|arg| {
            let given = value_in(fields, &arg.name)
                .or_else(|| value_in(typed, &arg.name))
                .unwrap_or("");
            (arg, given)
        })
        .collect();
    let rest = typed
        .iter()
        .filter(|(key, _)| !declared.iter().any(|arg| arg.name == *key))
        .cloned()
        .collect();
    (rest, values)
}

fn arg_kind(arg: &Arg) -> &'static str {
    match arg.kind {
        ArgKind::Scalar(Scalar::String) => "string",
        ArgKind::Scalar(Scalar::Number) => "number",
        ArgKind::Scalar(Scalar::Boolean) => "boolean",
        ArgKind::List(_) => "list",
        ArgKind::Map(_) => "map",
    }
}

fn pane(loaded: Result<Settings, String>) -> Value {
    match loaded {
        Err(error) => json!({ "error": error }),
        Ok(settings) => {
            let options: Vec<Value> = settings
                .options
                .iter()
                .map(|o| {
                    json!({
                        "group": o.group,
                        "name": o.name,
                        "label": o.label,
                        "description": o.description,
                        "kind": match o.kind {
                            Kind::Bool => "bool",
                            Kind::Number => "number",
                            Kind::Text => "text",
                            Kind::Choice => "choice",
                        },
                        "choices": o.choices,
                        "value": o.value,
                    })
                })
                .collect();
            json!({ "storage": settings.storage, "options": options })
        }
    }
}

fn skua_library(query: String) -> Reply {
    let home = match config::skua_home() {
        Ok(home) => home,
        Err(error) => return Reply::json(&json!({ "state": "failed", "error": error })),
    };
    let folder = library::scripts(&home);
    match library::open(&home) {
        Err(error) => Reply::json(&json!({ "state": "failed", "error": error })),
        Ok(None) => Reply::json(&json!({ "state": "missing", "folder": folder })),
        Ok(Some(found)) => {
            let entries: Vec<Value> = library::search(&found.entries, &query)
                .into_iter()
                .map(|e| {
                    json!({
                        "name": e.name,
                        "description": e.description,
                        "tags": e.tags,
                        "path": folder.join(&e.path),
                    })
                })
                .collect();
            Reply::json(&json!({
                "state": "ready",
                "commit": library::short(&found.commit),
                "folder": folder,
                "entries": entries,
            }))
        }
    }
}

fn resolve(home: &Path, typed: &str) -> PathBuf {
    let typed = typed.trim();
    let mut resolved = PathBuf::new();
    for part in home
        .join(scripts::expand_home(Path::new(typed)))
        .components()
    {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            part => resolved.push(part),
        }
    }
    resolved
}

fn write_private(path: &Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let name = path
        .file_name()
        .map_or_else(Default::default, |n| n.to_string_lossy().into_owned());
    let fresh = path.with_file_name(format!(".{name}.{}.{nanos}", std::process::id()));
    let written = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&fresh)
            .with_context(|| fresh.display().to_string())?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&fresh, &path).with_context(|| path.display().to_string())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&fresh);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_declared_arg_typed_as_key_value_fills_its_empty_field_once() {
        let declared = [
            Arg {
                name: "target".into(),
                kind: ArgKind::Scalar(Scalar::Number),
                optional: false,
                default: None,
            },
            Arg {
                name: "leader".into(),
                kind: ArgKind::Scalar(Scalar::String),
                optional: false,
                default: None,
            },
        ];
        let typed = pairs(&[("target", "7500"), ("leader", "player2"), ("fast", "true")]);
        let fields = pairs(&[("target", ""), ("leader", "player1")]);
        let (rest, values) = split_args(&declared, &typed, &fields);
        assert_eq!(rest, pairs(&[("fast", "true")]));
        let values: Vec<(&str, &str)> = values
            .iter()
            .map(|(arg, value)| (arg.name.as_str(), *value))
            .collect();
        assert_eq!(values, [("target", "7500"), ("leader", "player1")]);
    }

    #[test]
    fn skua_options_save_only_while_the_account_is_offline_or_idle() {
        let states = [
            State::Offline { why: None },
            State::Online,
            State::Busy(crate::view::Step::LoggingIn),
            State::Reconnecting(String::new()),
            State::Running,
            State::Paused,
            State::Stalled,
        ];
        let saves: Vec<bool> = states.iter().map(editable).collect();
        assert_eq!(saves, [true, true, false, false, false, false, false]);
    }
}
