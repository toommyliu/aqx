use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail};
use aqx_script::Cancel;
use aqx_script::skua::library::{self, Updated};
use aqx_script::skua::{self, SkuaScript};

use crate::config::{self, Script};
use crate::control::{Action, Want};
use crate::daemon::{self, Print, Session, Wanted};
use crate::fleet::{Channel, LastLines};
use crate::init;
use crate::view::{Shown, State, time_of_day};

const FOLLOW_EVERY: Duration = Duration::from_millis(250);

#[derive(clap::Args, Debug, Default)]
pub struct Open {
    /// Accounts to log in, by name or user. Without any, a new session logs in every account with auto_start on.
    #[arg(value_name = "ACCOUNT")]
    pub accounts: Vec<String>,
    /// The config to read instead of ./aqx.toml or ~/aqx/aqx.toml.
    #[arg(short, long, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Run this script on the accounts instead of their own.
    #[arg(long, value_name = "PATH")]
    pub script: Option<PathBuf>,
    /// A script argument. Repeatable.
    #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = crate::parse_arg, requires = "script")]
    pub args: Vec<(String, String)>,
}

impl Open {
    fn wanted(&self) -> anyhow::Result<Wanted> {
        let script = match &self.script {
            Some(path) => Some(Script {
                path: script_path(path)?,
                args: self.args.clone(),
            }),
            None => None,
        };
        Ok(Wanted {
            accounts: self.accounts.clone(),
            script,
        })
    }
}

#[derive(clap::Subcommand, Debug)]
pub enum Ctl {
    /// Log in, then run the account's script.
    Login,
    /// Log out.
    Logout,
    /// Stop the script.
    Stop,
    /// Run the last script again.
    Restart,
    /// Hold the script after its current step.
    Pause,
    /// Let a paused script go on.
    Resume,
    /// Log out and back in, then run the script again.
    Relogin,
    /// Run a script, stopping the one running. Logs the account in if it's offline.
    Run {
        /// The script, a .ts, .js or .cs file.
        script: PathBuf,
        /// A script argument. Repeatable.
        #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = crate::parse_arg)]
        args: Vec<(String, String)>,
    },
}

pub fn init(dir: Option<PathBuf>) -> anyhow::Result<ExitCode> {
    let found = || {
        let config = config::find(None).ok()?;
        std::path::absolute(config)
            .ok()?
            .parent()
            .map(Path::to_path_buf)
    };
    let dir = dir
        .or_else(found)
        .or_else(config::default_home)
        .ok_or_else(|| anyhow!("no home folder; pass the folder to write into"))?;
    init::init(&dir, init::ask).map_err(anyhow::Error::msg)?;
    Ok(ExitCode::SUCCESS)
}

fn config_path(given: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = config::find(given).map_err(anyhow::Error::msg)?;
    Ok(std::path::absolute(path)?)
}

fn script_path(script: &Path) -> anyhow::Result<PathBuf> {
    let path = std::path::absolute(script)?;
    if !path.is_file() {
        bail!("{} isn't a file", path.display());
    }
    Ok(path)
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

fn confirm(question: &str) -> anyhow::Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

enum Joined {
    Started,
    Attached,
    Restarted,
}

fn session_for(path: &Path, wanted: &Wanted) -> anyhow::Result<(Session, Joined)> {
    let Some(session) = daemon::session_for(path)? else {
        return Ok((daemon::start(path, wanted)?, Joined::Started));
    };
    if !session.compatible() {
        eprintln!(
            "aqx is already running for this config from an incompatible build. Restarting it with this one; its accounts log out and back in."
        );
        return Ok((restart(session, path, wanted)?, Joined::Restarted));
    }
    if !session.same_build() {
        if !std::io::stdin().is_terminal() {
            bail!(
                "aqx is already running for this config from a different build; run aqx down and start it again"
            );
        }
        if confirm(
            "aqx is already running for this config from a different build. Restart it with this one? Its accounts log out and back in.",
        )? {
            return Ok((restart(session, path, wanted)?, Joined::Restarted));
        }
    }
    if wanted.accounts.is_empty() && wanted.script.is_some() {
        bail!("aqx is already running for this config; name the accounts to run the script on");
    }
    act_on_named(&session, wanted)?;
    Ok((session, Joined::Attached))
}

fn restart(old: Session, path: &Path, wanted: &Wanted) -> anyhow::Result<Session> {
    let mut resume = old.resumable().unwrap_or_default();
    match resume.len() {
        0 => eprintln!("Ending the old session…"),
        1 => eprintln!("Logging out 1 account…"),
        n => eprintln!("Logging out {n} accounts…"),
    }
    resume.retain(|(name, _)| !wanted.accounts.iter().any(|w| w.eq_ignore_ascii_case(name)));
    old.down()?;
    eprintln!("Starting the new session…");
    let mut resume = resume.into_iter();
    let Some((first, script)) = resume.next() else {
        return daemon::start(path, wanted);
    };
    let fresh = daemon::start(
        path,
        &Wanted {
            accounts: vec![first],
            script,
        },
    )?;
    let accounts = names(&fresh)?;
    for (name, script) in resume {
        if let Some(i) = accounts.iter().position(|a| a.name == name) {
            let _ = fresh
                .source()
                .act(i, script.map_or(Action::Login, Action::Run));
        }
    }
    act_on_named(&fresh, wanted)?;
    Ok(fresh)
}

fn attached_note(session: &Session, path: &Path) -> String {
    let since = time_of_day(session.started_ms);
    let since = since.split('.').next().unwrap_or(&since).to_owned();
    let changed = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|t| t.as_millis() as u64 > session.started_ms);
    if changed {
        format!(
            "attached to the session running since {since}; aqx.toml changed after it started, so run aqx down, then aqx up -d, to apply it"
        )
    } else {
        format!("attached to the session running since {since}")
    }
}

fn act_on_named(session: &Session, wanted: &Wanted) -> anyhow::Result<()> {
    let accounts = names(session)?;
    for name in &wanted.accounts {
        let i = find(&accounts, name)?;
        let action = match &wanted.script {
            Some(script) => Action::Run(script.clone()),
            None => Action::Login,
        };
        if let Err(why) = session.source().act(i, action) {
            eprintln!("aqx: {}: {why}", accounts[i].name);
        }
    }
    Ok(())
}

pub fn up_detached(open: &Open) -> anyhow::Result<ExitCode> {
    let path = config_path(open.config.as_deref())?;
    config::load(&path).map_err(anyhow::Error::msg)?;
    let (session, joined) = session_for(&path, &open.wanted()?)?;
    match joined {
        Joined::Started => println!("started an aqx session for {}", session.config.display()),
        Joined::Attached => println!(
            "aqx is already running for {}; {}",
            session.config.display(),
            attached_note(&session, &path)
        ),
        Joined::Restarted => println!(
            "restarted the aqx session for {} with this build",
            session.config.display()
        ),
    }
    Ok(ExitCode::SUCCESS)
}

pub fn up(open: &Open, json: bool, quiet: bool) -> anyhow::Result<ExitCode> {
    let path = config_path(open.config.as_deref())?;
    let print = match (json, quiet) {
        (true, _) => Print::Json,
        (_, true) => Print::Quiet,
        _ => Print::Plain,
    };
    daemon::serve(&path, &open.wanted()?, print)?;
    Ok(ExitCode::SUCCESS)
}

pub fn web(config: Option<&Path>, port: u16, up: bool) -> anyhow::Result<ExitCode> {
    let path = config_path(config)?;
    config::load(&path).map_err(anyhow::Error::msg)?;
    crate::web::serve(path, port, up)?;
    Ok(ExitCode::SUCCESS)
}

pub fn down(config: Option<&Path>) -> anyhow::Result<ExitCode> {
    let path = config_path(config)?;
    let Some(session) = daemon::session_for(&path)? else {
        println!("aqx isn't running for {}", path.display());
        return Ok(ExitCode::SUCCESS);
    };
    println!("logging out every account in {}", path.display());
    print_lines(&last_words(&session.down()?));
    println!("the aqx session ended");
    Ok(ExitCode::SUCCESS)
}

fn last_words(accounts: &[LastLines]) -> Vec<String> {
    let tail = if accounts.len() == 1 { 10 } else { 3 };
    let width = accounts
        .iter()
        .map(|a| a.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    for account in accounts {
        let who = if accounts.len() == 1 {
            String::new()
        } else {
            format!("{:<width$}  ", account.name)
        };
        let lines = &account.lines[account.lines.len().saturating_sub(tail)..];
        for line in lines {
            out.push(format!(
                "{}  {who}{:<6}  {}",
                time_of_day(line.at_ms),
                line.channel.label(),
                line.text
            ));
        }
    }
    out
}

fn names(session: &Session) -> anyhow::Result<Vec<Shown>> {
    let want = Want {
        selected: usize::MAX,
        since: u64::MAX,
        full: false,
    };
    Ok(session
        .source()
        .frame(&want)
        .map_err(anyhow::Error::msg)?
        .accounts)
}

fn find(accounts: &[Shown], wanted: &str) -> anyhow::Result<usize> {
    accounts
        .iter()
        .position(|a| a.name.eq_ignore_ascii_case(wanted) || a.user.eq_ignore_ascii_case(wanted))
        .ok_or_else(|| {
            let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
            anyhow!("no account {wanted}; the accounts are {}", names.join(", "))
        })
}

fn incompatible(session: &Session) -> String {
    format!(
        "the session for {} was started by an incompatible build of aqx; run aqx down -c {} and start it again",
        session.config.display(),
        session.config.display()
    )
}

fn find_anywhere(wanted: &str) -> anyhow::Result<(Session, usize)> {
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for session in daemon::sessions()? {
        if !session.compatible() {
            skipped.push(incompatible(&session));
            continue;
        }
        let accounts = names(&session)?;
        if let Ok(i) = find(&accounts, wanted) {
            found.push((session, i));
        }
    }
    match found.len() {
        0 if !skipped.is_empty() => bail!(
            "no running aqx session this build can read has an account {wanted}; {}",
            skipped.join("; ")
        ),
        0 => bail!("no running aqx session has an account {wanted}"),
        1 => Ok(found.remove(0)),
        _ => bail!("{wanted} is in more than one running aqx session"),
    }
}

fn session_label(session: &Session) -> String {
    session
        .config
        .parent()
        .and_then(|dir| dir.file_name())
        .map_or_else(
            || session.config.display().to_string(),
            |dir| dir.to_string_lossy().into_owned(),
        )
}

pub fn ps(json: bool) -> anyhow::Result<ExitCode> {
    let sessions = daemon::sessions()?;
    if sessions.is_empty() {
        println!("no aqx session is running");
        return Ok(ExitCode::SUCCESS);
    }
    let mut rows = Vec::new();
    for session in &sessions {
        if !session.compatible() {
            eprintln!("aqx: {}", incompatible(session));
            continue;
        }
        for account in names(session)? {
            if json {
                let mut value = serde_json::to_value(&account)?;
                value["config"] = serde_json::json!(session.config);
                println!("{value}");
            } else {
                rows.push((session_label(session), account));
            }
        }
    }
    if !json {
        print_lines(&ps_lines(&rows));
    }
    Ok(ExitCode::SUCCESS)
}

fn ps_lines(rows: &[(String, Shown)]) -> Vec<String> {
    let header = [
        "SESSION", "ACCOUNT", "STATE", "SCRIPT", "WHERE", "UP", "STATUS",
    ]
    .map(str::to_owned);
    let mut table = vec![header];
    for (session, a) in rows {
        let script = a
            .script
            .as_deref()
            .map(|s| s.rsplit('/').next().unwrap_or(s).to_owned())
            .unwrap_or_default();
        let place = a
            .game
            .as_ref()
            .map(|g| format!("{} {}", g.area, g.cell))
            .unwrap_or_default();
        let up = a.up.map(|d| span(d.as_secs())).unwrap_or_default();
        let status = match &a.state {
            State::Offline { why: Some(why) } => why.clone(),
            State::Reconnecting(detail) => detail.clone(),
            _ => a.status.clone(),
        };
        table.push([
            session.clone(),
            a.name.clone(),
            a.state.word().to_owned(),
            script,
            place,
            up,
            status,
        ]);
    }
    let widths: Vec<usize> = (0..7)
        .map(|col| {
            table
                .iter()
                .map(|row| row[col].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    table
        .iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, &width)| format!("{cell:width$}"))
                .collect();
            cells.join("  ").trim_end().to_owned()
        })
        .collect()
}

fn span(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h{}m", secs / 3600, secs % 3600 / 60),
    }
}

pub fn logs(account: Option<&str>, follow: bool, all: bool) -> anyhow::Result<ExitCode> {
    let (session, targets) = match account {
        Some(wanted) => {
            let (session, i) = find_anywhere(wanted)?;
            (session, vec![i])
        }
        None => {
            let mut sessions = daemon::sessions()?;
            match sessions.len() {
                0 => bail!("no aqx session is running"),
                1 => {
                    let session = sessions.remove(0);
                    if !session.compatible() {
                        bail!(incompatible(&session));
                    }
                    let count = names(&session)?.len();
                    (session, (0..count).collect())
                }
                _ => {
                    let listed: Vec<String> = sessions
                        .iter()
                        .map(|s| s.config.display().to_string())
                        .collect();
                    bail!(
                        "more than one aqx session is running ({}); name an account",
                        listed.join(", ")
                    );
                }
            }
        }
    };
    let shown = names(&session)?;
    let width = targets
        .iter()
        .map(|&i| shown[i].name.chars().count())
        .max()
        .unwrap_or(0);
    let mut since = vec![0; shown.len()];
    loop {
        for &i in &targets {
            let want = Want {
                selected: i,
                since: since[i],
                full: false,
            };
            let frame = session.source().frame(&want).map_err(anyhow::Error::msg)?;
            for line in frame.log {
                since[i] = line.seq + 1;
                if !all && line.channel == Channel::Game {
                    continue;
                }
                println!(
                    "{}  {:width$}  {:<6}  {}",
                    time_of_day(line.at_ms),
                    shown[i].name,
                    line.channel.label(),
                    line.text
                );
            }
            if frame.ending {
                return Ok(ExitCode::SUCCESS);
            }
        }
        if !follow {
            return Ok(ExitCode::SUCCESS);
        }
        thread::sleep(FOLLOW_EVERY);
    }
}

pub fn ctl(account: &str, ctl: Ctl) -> anyhow::Result<ExitCode> {
    let (session, i) = find_anywhere(account)?;
    let name = names(&session)?.swap_remove(i).name;
    let (action, done) = match ctl {
        Ctl::Login => (Action::Login, "logging in".to_owned()),
        Ctl::Logout => (Action::Logout, "logging out".to_owned()),
        Ctl::Stop => (Action::Stop, "stopping the script".to_owned()),
        Ctl::Restart => (Action::Restart, "restarting the script".to_owned()),
        Ctl::Pause => (Action::Pause, "paused".to_owned()),
        Ctl::Resume => (Action::Resume, "resumed".to_owned()),
        Ctl::Relogin => (Action::Relogin, "logging out to log in again".to_owned()),
        Ctl::Run { script, args } => {
            let path = script_path(&script)?;
            let done = format!("running {}", path.display());
            (Action::Run(Script { path, args }), done)
        }
    };
    match session.source().act(i, action) {
        Ok(()) => {
            println!("{name}: {done}");
            Ok(ExitCode::SUCCESS)
        }
        Err(why) => {
            println!("{name}: {why}");
            Ok(ExitCode::SUCCESS)
        }
    }
}

#[derive(clap::Subcommand, Debug)]
pub enum Skua {
    /// Download the Skua scripts if they changed upstream, then build the Skua worker.
    Update,
    /// List the downloaded Skua scripts, or the ones matching every word of QUERY. A #word matches that tag, or the tags starting with it when no tag has that name.
    List { query: Vec<String> },
    /// Print the folder that holds the Skua scripts and their options.
    Path,
}

pub fn skua(action: Skua) -> anyhow::Result<ExitCode> {
    let home = config::skua_home().map_err(|e| anyhow!(e))?;
    match action {
        Skua::Path => println!("{}", home.display()),
        Skua::Update => {
            match library::update(&home).map_err(|e| anyhow!(e))? {
                Updated::Current => println!("the Skua scripts are up to date"),
                Updated::To(commit) => println!(
                    "updated the Skua scripts to {} in {}",
                    library::short(&commit),
                    library::scripts(&home).display()
                ),
            }
            skua::worker(&mut || println!("building the Skua worker")).map_err(|e| anyhow!(e))?;
        }
        Skua::List { query } => {
            let Some(found) = library::open(&home).map_err(|e| anyhow!(e))? else {
                bail!("the Skua scripts aren't downloaded. Run `aqx skua update`");
            };
            let query = query.join(" ");
            let scripts = library::scripts(&home);
            let mut out = std::io::stdout().lock();
            for entry in library::search(&found.entries, &query) {
                let path = scripts.join(&entry.path);
                if writeln!(out, "{}  {}", path.display(), entry.name).is_err() {
                    break;
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn check(scripts: &[PathBuf]) -> anyhow::Result<ExitCode> {
    let mut failed = 0;
    for script in scripts {
        let checked = if script
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cs"))
        {
            config::skua_home().and_then(|skua| {
                SkuaScript::load(script, &skua, |_| {}, &Cancel::default())
                    .map(drop)
                    .map_err(|e| e.to_string())
            })
        } else {
            aqx_script::check(script)
        };
        match checked {
            Ok(()) => println!("{}: ok", script.display()),
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", script.display());
            }
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Game;

    fn shown(name: &str, state: State) -> Shown {
        Shown {
            name: name.into(),
            user: name.into(),
            state,
            server: String::new(),
            script: None,
            status: String::new(),
            up: None,
            stats: crate::fleet::Stats::default(),
            gold_gained: None,
            game: None,
        }
    }

    #[test]
    fn ps_lists_each_account_under_its_session() {
        let farming = Shown {
            server: "Twilly".into(),
            script: Some("/srv/farm/void-aura.ts".into()),
            status: "Void Aura 12/7500".into(),
            up: Some(Duration::from_secs(4000)),
            game: Some(Game {
                area: "farm-1".into(),
                cell: "Enter".into(),
                pad: "Spawn".into(),
                ..Game::default()
            }),
            ..shown("player1", State::Running)
        };
        let banned = shown(
            "healer",
            State::Offline {
                why: Some("login rejected: banned".into()),
            },
        );
        let lines = ps_lines(&[("farm".into(), farming), ("farm".into(), banned)]);
        assert_eq!(
            lines,
            [
                "SESSION  ACCOUNT  STATE    SCRIPT        WHERE         UP    STATUS",
                "farm     player1  running  void-aura.ts  farm-1 Enter  1h6m  Void Aura 12/7500",
                "farm     healer   offline                                    login rejected: banned",
            ]
        );
        let accounts = [
            shown("player1", State::Online),
            shown("healer", State::Online),
        ];
        assert_eq!(find(&accounts, "HEALER").unwrap(), 1);
        assert!(
            find(&accounts, "nobody")
                .unwrap_err()
                .to_string()
                .contains("the accounts are player1, healer")
        );
    }

    #[test]
    fn down_prints_the_last_lines_of_each_account() {
        let accounts = ["player1", "player2"]
            .map(|name| std::sync::Arc::new(crate::fleet::Account::detached(name)));
        for (channel, text) in [
            (Channel::Aqx, "logging in to Twilly"),
            (Channel::Script, "farming"),
            (Channel::Aqx, "the script stopped: done"),
            (Channel::Game, "killed Frogzard, +10 gold, +5 xp"),
            (Channel::Aqx, "disconnected: logged out"),
        ] {
            accounts[0].board().log.push(channel, text.into());
        }
        accounts[1]
            .board()
            .log
            .push(Channel::Aqx, "couldn't log in: player2 is banned".into());
        let fleet = crate::fleet::Fleet::of(accounts.to_vec());
        let words = last_words(&fleet.last_lines());
        let expected = [
            "farming",
            "the script stopped: done",
            "disconnected: logged out",
            "couldn't log in: player2 is banned",
        ];
        assert_eq!(
            words.len(),
            expected.len(),
            "the last 3 lines of each, without game events: {words:?}"
        );
        assert!(
            words
                .iter()
                .zip(expected)
                .all(|(w, e)| w.ends_with(&format!("  {e}"))),
            "{words:?}"
        );
        assert!(words[0].contains("  player1  script  farming"), "{words:?}");

        let one = crate::fleet::Fleet::of(accounts[1..].to_vec());
        assert!(
            !last_words(&one.last_lines())[0].contains("player2  aqx"),
            "one account needs no name column"
        );
    }
}
