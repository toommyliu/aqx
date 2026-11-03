mod account;
mod config;
#[cfg(test)]
#[path = "../../aqx-core/tests/server/mod.rs"]
mod fake;
mod hub;
mod init;
mod output;
mod prompt;
mod socket;
mod tui;
mod view;

use std::hash::{Hash, Hasher};
use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aqx_core::{CLIENT_VERSION, Control, Credentials, Recorder};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use account::{Backoff, Script, Spec};
use config::{AccountConfig, Config, Settings};
use hub::{AccountCell, Cells, Command, Hub, Print};
use socket::Conn;

const DEFAULT_API: &str = "https://game.aq.com";
const STALL_AFTER: Duration = Duration::from_secs(300);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const DOWN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(name = "aqx", version, about = "Headless AQW client. Without a command, opens the TUI", args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
    #[command(flatten)]
    open: Wanted,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = "Log one account in and print what it does until its script ends, or until Ctrl-C without one")]
    Run(RunArgs),
    #[command(about = "Run the session in the foreground, for systemd. aqx starts one in the background")]
    Serve {
        #[command(flatten)]
        wanted: Wanted,
        #[arg(long, help = "Print events as JSON lines")]
        json: bool,
        #[arg(long, conflicts_with = "json", help = "Print nothing; read the log with aqx logs or the TUI")]
        quiet: bool,
    },
    #[command(about = "Log out every account and end the session")]
    Down {
        #[arg(short, long, value_name = "FILE", help = "Config file")]
        config: Option<PathBuf>,
    },
    #[command(about = "List running aqx processes and their accounts")]
    Ps,
    #[command(about = "Print an account's recent log")]
    Logs {
        #[arg(help = "Only this account")]
        account: Option<String>,
        #[arg(short, long, help = "Keep printing new lines")]
        follow: bool,
        #[arg(long, help = "Include game lines: moves, kills, drops, items and quests")]
        all: bool,
    },
    #[command(about = "Control an account in a running aqx process")]
    Ctl {
        account: String,
        #[command(subcommand)]
        action: Ctl,
    },
    #[command(about = "List the game servers")]
    Servers {
        #[arg(short, long, value_name = "FILE", help = "Config file, for its api")]
        config: Option<PathBuf>,
    },
    #[command(about = "Print a --record file one packet per line")]
    View(ViewArgs),
    #[command(about = "Print the JSON Schema of aqx.toml, for editor completion and checks")]
    Schema,
    #[command(about = "Write aqx.d.ts, tsconfig.json, a sample script and, in a terminal, an aqx.toml for one account. Rerun after upgrading aqx to update aqx.d.ts")]
    Init {
        #[arg(default_value = ".", help = "Directory to write into")]
        dir: PathBuf,
    },
    #[command(about = "Compile scripts and the files they import, without logging in or running them")]
    Check {
        #[arg(required = true)]
        scripts: Vec<PathBuf>,
    },
}

#[derive(Subcommand)]
enum Ctl {
    #[command(about = "Log in and run the account's own script")]
    Login,
    #[command(about = "Run a script, stopping the one running. Logs the account in if it's offline")]
    Run {
        script: PathBuf,
        #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = parse_arg)]
        args: Vec<(String, String)>,
    },
    Stop,
    Restart,
    Pause,
    Resume,
    Relogin,
    Logout,
}

#[derive(Args)]
struct Wanted {
    #[arg(value_name = "ACCOUNT", help = "Accounts to log in, by name or user. Without any, a new session logs in every account")]
    accounts: Vec<String>,
    #[arg(short, long, value_name = "FILE", help = "Config file")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "FILE", help = "Run this script on the accounts instead of their own")]
    script: Option<PathBuf>,
    #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = parse_arg, requires = "script", help = "Script input read by script.args(). Repeatable")]
    args: Vec<(String, String)>,
}

#[derive(Args)]
struct ViewArgs {
    file: PathBuf,
    #[arg(long, value_name = "NAME", help = "Only show these commands, e.g. --cmd loginResponse --cmd joinOK")]
    cmd: Vec<String>,
    #[arg(long, help = "Don't cut lines at 160 characters")]
    full: bool,
}

#[derive(Args)]
struct RunArgs {
    #[arg(help = "Script to run instead of the account's own. An account without one stays online until Ctrl-C")]
    script: Option<PathBuf>,
    #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = parse_arg, help = "Script input read by script.args(), over the account's args. Repeatable")]
    args: Vec<(String, String)>,
    #[arg(long, conflicts_with_all = ["script", "args"], help = "Log in without running the account's script")]
    idle: bool,
    #[arg(short, long, value_name = "NAME", help = "The [[account]] to use from the config, by name or user")]
    account: Option<String>,
    #[arg(short, long, value_name = "FILE", help = "Config file")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "NAME", help = "Server to join, e.g. Twilly")]
    server: Option<String>,
    #[arg(long, value_name = "FILE", help = "Write every packet to FILE as JSON lines")]
    record: Option<PathBuf>,
    #[arg(long, value_name = "VERSION", default_value = CLIENT_VERSION, help = "Client version to log in with first")]
    client_version: String,
    #[arg(long, help = "Print events as JSON lines")]
    json: bool,
}

fn parse_arg(arg: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.into(), value.into())),
        _ => Err("expected KEY=VALUE".into()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => open(&cli.open),
        Some(Cmd::Run(args)) => run(args),
        Some(Cmd::Serve { wanted, json, quiet }) => serve(&wanted, json, quiet),
        Some(Cmd::Down { config }) => down(config.as_deref()),
        Some(Cmd::Ps) => ps(),
        Some(Cmd::Logs { account, follow, all }) => logs(account.as_deref(), follow, all),
        Some(Cmd::Ctl { account, action }) => ctl(&account, action),
        Some(Cmd::Servers { config }) => servers(config.as_deref()),
        Some(Cmd::View(args)) => view::view(&args.file, &args.cmd, args.full),
        Some(Cmd::Schema) => {
            println!("{}", config::schema());
            Ok(())
        }
        Some(Cmd::Init { dir }) => init::init(&dir, &config::user_config(), init::ask),
        Some(Cmd::Check { scripts }) => check(&scripts),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aqx: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Launch {
    name: String,
    spec: Spec,
    recorder: Option<Recorder>,
}

fn spec(account: &AccountConfig, settings: Settings, version: &str, script: Option<Script>, exit_when_done: bool) -> Spec {
    Spec {
        creds: Credentials { api: settings.api.unwrap_or_else(|| DEFAULT_API.into()), user: account.user.clone(), password: account.password.clone() },
        server: settings.server,
        version: version.into(),
        script,
        room: settings.room,
        restart: settings.restart.unwrap_or_default(),
        stall_after: settings.stall_after.unwrap_or(STALL_AFTER),
        on_stall: settings.on_stall.unwrap_or_default(),
        exit_when_done,
        relogin: Backoff::new(account::RELOGIN_FIRST, account::RELOGIN_MAX),
        restarts: Backoff::new(account::RESTART_FIRST, account::RELOGIN_MAX),
    }
}

fn run(args: RunArgs) -> Result<(), String> {
    let RunArgs { script, args, idle, account: name, config, server, record, client_version, json } = args;
    let (config, path) = config::load(config.as_deref())?;
    let account = config.account(name.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    let script = account.run_script(&path, script, args, idle)?;
    let recorder = record.map(|p| Recorder::create(&p).map_err(|e| format!("{}: {e}", p.display()))).transpose()?;
    let mut settings = account.settings(&config.defaults);
    settings.server = server.or(settings.server);
    let exit_when_done = script.is_some();
    let launch = Launch { name: account.name().into(), spec: spec(account, settings, &client_version, script, exit_when_done), recorder };
    let print = if json { Print::Json } else { Print::Plain };
    let results = start(account.name(), None, vec![launch], print, &|_| Some(Command::Login))?;
    results.into_iter().next().map_or(Ok(()), |(_, r)| r)
}

fn open(wanted: &Wanted) -> Result<(), String> {
    if !std::io::stdout().is_terminal() {
        return Err("aqx opens a TUI and needs a terminal; use aqx serve or aqx run".into());
    }
    let (config, path) = config::load(wanted.config.as_deref())?;
    let accounts = named(&config, &wanted.accounts)?;
    let command = command(wanted)?;
    let session = match socket::session_for(&path)? {
        None => start_session(&path, &accounts, &command)?,
        Some(session) => {
            let info = socket::info(&session)?;
            if socket::other_build(&info) && restart_wanted(&info)? {
                restart_session(&session, &path, &accounts, &command)?
            } else {
                socket::same_build(&info)?;
                if accounts.is_empty() && wanted.script.is_some() {
                    return Err("aqx is already running for this config; name the accounts to run the script on".into());
                }
                for account in &accounts {
                    socket::ctl(&session, account, &command)?;
                }
                session
            }
        }
    };
    attach(&session)
}

fn restart_wanted(info: &serde_json::Value) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        socket::same_build(info)?;
        return Err("aqx is already running for this config from a different build; run aqx down and start it again".into());
    }
    prompt::confirm("aqx is already running for this config from a different build. Restart it with this one? Its accounts log out and back in.")
}

fn restart_session(session: &Path, config: &Path, accounts: &[String], command: &Command) -> Result<PathBuf, String> {
    let mut resume = socket::to_resume(session)?;
    resume.retain(|(account, _)| !accounts.contains(account));
    resume.extend(accounts.iter().map(|account| (account.clone(), command.clone())));
    end_session(session)?;
    let mut resume = resume.into_iter();
    let Some((first, command)) = resume.next() else { return start_session(config, accounts, command) };
    let session = start_session(config, &[first], &command)?;
    for (account, command) in resume {
        socket::ctl(&session, &account, &command)?;
    }
    Ok(session)
}

fn named(config: &Config, names: &[String]) -> Result<Vec<String>, String> {
    let mut accounts: Vec<String> = Vec::new();
    for name in names {
        let account = config.account(Some(name))?.name().to_string();
        if !accounts.contains(&account) {
            accounts.push(account);
        }
    }
    Ok(accounts)
}

fn command(wanted: &Wanted) -> Result<Command, String> {
    let Some(script) = &wanted.script else { return Ok(Command::Login) };
    Ok(Command::Run { script: account::script_path(script)?, args: wanted.args.clone() })
}

fn start_session(config: &Path, accounts: &[String], command: &Command) -> Result<PathBuf, String> {
    let stem = config.file_stem().map_or("aqx".into(), |s| s.to_string_lossy().into_owned());
    let mut hasher = std::hash::DefaultHasher::new();
    config.hash(&mut hasher);
    let log = socket::prepare_dir()?.join(format!("{stem}-{:016x}.log", hasher.finish()));
    // The session being replaced can still hold this log open; a fresh file keeps its last writes out.
    let _ = std::fs::remove_file(&log);
    let errors = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut serve = std::process::Command::new(exe);
    serve.args(["serve", "--quiet", "--config"]).arg(config).args(accounts);
    if let Command::Run { script, args } = command {
        serve.arg("--script").arg(script);
        for (key, value) in args {
            serve.arg("--arg").arg(format!("{key}={value}"));
        }
    }
    serve.stdin(Stdio::null()).stdout(Stdio::null()).stderr(errors);
    unsafe {
        serve.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = serve.spawn().map_err(|e| format!("couldn't start aqx serve: {e}"))?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(session) = socket::session_for(config)? {
            return Ok(session);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let said = std::fs::read_to_string(&log).unwrap_or_default();
            let said = said.trim();
            return Err(if said.is_empty() { format!("aqx serve exited ({status})") } else { said.trim_start_matches("aqx: ").to_string() });
        }
        if Instant::now() >= deadline {
            return Err(format!("the session didn't start within {}s; see {}", START_TIMEOUT.as_secs(), log.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn serve(wanted: &Wanted, json: bool, quiet: bool) -> Result<(), String> {
    let (config, path) = config::load(wanted.config.as_deref())?;
    if config.accounts.is_empty() {
        return Err(format!("{} has no [[account]]", path.display()));
    }
    if let Some(session) = socket::session_for(&path)? {
        return Err(format!("aqx is already running for {}; its socket is {}", path.display(), session.display()));
    }
    let accounts = named(&config, &wanted.accounts)?;
    let command = command(wanted)?;
    let launches = launches(&config, &path)?;
    let name = path.file_stem().map_or("aqx".into(), |s| s.to_string_lossy().into_owned());
    let print = match (json, quiet) {
        (true, _) => Print::Json,
        (_, true) => Print::Quiet,
        _ => Print::Plain,
    };
    let first = |account: &str| (accounts.is_empty() || accounts.iter().any(|a| a == account)).then(|| command.clone());
    let results = start(&name, Some(&path), launches, print, &first)?;
    let failed: Vec<String> = results.into_iter().filter_map(|(account, r)| r.err().map(|e| format!("{account}: {e}"))).collect();
    if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }
}

fn launches(config: &Config, path: &Path) -> Result<Vec<Launch>, String> {
    let mut launches = Vec::new();
    for account in &config.accounts {
        let spec = spec(account, account.settings(&config.defaults), CLIENT_VERSION, account.script(path)?, false);
        launches.push(Launch { name: account.name().into(), spec, recorder: None });
    }
    Ok(launches)
}

fn down(config: Option<&Path>) -> Result<(), String> {
    let (_, path) = config::load(config)?;
    let session = socket::session_for(&path)?.ok_or_else(|| format!("aqx isn't running for {}", path.display()))?;
    end_session(&session)
}

fn end_session(session: &Path) -> Result<(), String> {
    for account in socket::accounts(session)? {
        socket::ctl(session, &account, &Command::Logout)?;
    }
    let deadline = Instant::now() + DOWN_TIMEOUT;
    while std::os::unix::net::UnixStream::connect(session).is_ok() {
        if Instant::now() >= deadline {
            return Err(format!("the session is still logging out after {}s", DOWN_TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

type Outcomes = Vec<(String, Result<(), String>)>;

struct Running {
    name: String,
    user: String,
    thread: JoinHandle<Result<(), String>>,
}

fn start(name: &str, config: Option<&Path>, launches: Vec<Launch>, print: Print, first: &dyn Fn(&str) -> Option<Command>) -> Result<Outcomes, String> {
    let (name, listener, _listening) = socket::bind(name, true)?;
    let hub = Arc::new(Hub::new(&name, Vec::new(), print));
    let mut running = launches.into_iter().map(|launch| run_account(&hub, launch, first)).collect::<Result<Vec<_>, String>>()?;
    socket::serve(listener, hub.clone(), config.map(Path::to_path_buf))?;
    let presses = Arc::new(AtomicUsize::new(0));
    {
        let hub = hub.clone();
        ctrlc::set_handler(move || {
            if presses.fetch_add(1, Ordering::SeqCst) > 0 {
                std::process::exit(130);
            }
            log_out_all(&hub);
        })
        .map_err(|e| e.to_string())?;
    }
    let mut seen = config.and_then(modified);
    while !hub.end_if_idle() {
        std::thread::sleep(Duration::from_millis(250));
        let Some(config) = config else { continue };
        if modified(config) == seen {
            continue;
        }
        seen = modified(config);
        match added_accounts(config, &running) {
            Ok(launches) => {
                for launch in launches {
                    running.push(run_account(&hub, launch, &|_| None)?);
                }
            }
            Err(e) => eprintln!("aqx: not adding accounts from {}: {e}", config.display()),
        }
    }
    for cell in hub.cells.all() {
        cell.close();
    }
    Ok(running.into_iter().map(|r| (r.name, r.thread.join().unwrap_or_else(|_| Err("the account thread crashed".into())))).collect())
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn added_accounts(path: &Path, running: &[Running]) -> Result<Vec<Launch>, String> {
    let (config, path) = config::load(Some(path))?;
    let known = |launch: &Launch| running.iter().any(|r| r.name == launch.name || r.user.eq_ignore_ascii_case(&launch.spec.creds.user));
    Ok(launches(&config, &path)?.into_iter().filter(|launch| !known(launch)).collect())
}

fn run_account(hub: &Arc<Hub>, launch: Launch, first: &dyn Fn(&str) -> Option<Command>) -> Result<Running, String> {
    let control = Control::new().map_err(|e| e.to_string())?;
    let cell = Arc::new(AccountCell::local(&launch.name, control.stop_handle()));
    if let Some(command) = first(&launch.name) {
        cell.send(command)?;
    }
    hub.cells.add(cell.clone());
    let hub = hub.clone();
    let (name, user) = (launch.name.clone(), launch.spec.creds.user.clone());
    let thread = std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || account::run(launch.spec, launch.recorder, control, cell, hub))
        .map_err(|e| e.to_string())?;
    Ok(Running { name, user, thread })
}

fn log_out_all(hub: &Hub) {
    for cell in hub.cells.all() {
        let _ = cell.send(Command::Logout);
    }
}

fn attach(path: &Path) -> Result<(), String> {
    let mut conn = Conn::open(path)?;
    conn.send(&json!({ "op": "subscribe" }))?;
    let hello = conn.recv().ok_or("the aqx process closed the connection")?;
    socket::same_build(&hello)?;
    let name = hello["name"].as_str().unwrap_or("aqx").to_string();
    let accounts: Vec<String> = serde_json::from_value(hello["accounts"].clone()).map_err(|e| e.to_string())?;
    let cells = Arc::new(Cells::new(accounts.iter().map(|name| Arc::new(AccountCell::mirror(name))).collect()));
    let gone = Arc::new(AtomicBool::new(false));
    {
        let (cells, gone) = (cells.clone(), gone.clone());
        std::thread::spawn(move || {
            while let Some(message) = conn.recv() {
                mirror(&cells, &message);
            }
            gone.store(true, Ordering::SeqCst);
        });
    }
    let fleet = tui::Fleet { title: name, cells, socket: path.to_path_buf() };
    tui::run(&fleet, || gone.load(Ordering::SeqCst))?;
    if gone.load(Ordering::SeqCst)
        && let [only] = &fleet.cells.all()[..]
    {
        let board = only.board();
        for line in board.log.iter().skip(board.log.len().saturating_sub(10)) {
            println!("{}  {:<6}  {}", output::time_of_day(line.t), line.source, line.text);
        }
    }
    Ok(())
}

fn mirror(cells: &Cells, message: &serde_json::Value) {
    let find = |account: &str| cells.all().into_iter().find(|c| c.name == account);
    match message["type"].as_str() {
        Some("account") => {
            if let Some(account) = message["account"].as_str()
                && find(account).is_none()
            {
                cells.add(Arc::new(AccountCell::mirror(account)));
            }
        }
        Some("log") => {
            let (Some(account), Ok(line)) = (message["account"].as_str(), serde_json::from_value::<hub::LogLine>(message["line"].clone())) else { return };
            if let Some(cell) = find(account) {
                cell.board().log.push(line);
            }
        }
        Some("snapshot") => {
            let Ok(snapshot) = serde_json::from_value::<hub::Snapshot>(message["snapshot"].clone()) else { return };
            if let Some(cell) = find(&snapshot.account) {
                cell.board().snapshot = Some(snapshot);
            }
        }
        _ => {}
    }
}

fn ps() -> Result<(), String> {
    let all = socket::processes()?;
    if all.is_empty() {
        println!("no aqx process is running");
        return Ok(());
    }
    println!("{:<12}  {:<10}  {:<12}  {:<22}  {:<24}  {:<6}  STATUS", "PROCESS", "ACCOUNT", "STATE", "SCRIPT", "WHERE", "UP");
    for (name, path) in all {
        let accounts = match socket::snapshot(&path) {
            Ok(accounts) => accounts,
            Err(e) => {
                println!("{name:<12}  {e}");
                continue;
            }
        };
        for s in accounts {
            let (state, script) = (s.state(), s.script_label());
            let place = s.game.as_ref().map_or(String::new(), |g| format!("{} {}", g.area, g.cell));
            let status = if s.phase != "READY" && !s.detail.is_empty() { &s.detail } else { &s.status };
            println!("{name:<12}  {:<10}  {state:<12}  {script:<22}  {place:<24}  {:<6}  {status}", s.account, output::span(s.up_secs));
        }
    }
    Ok(())
}

fn logs(account: Option<&str>, follow: bool, all: bool) -> Result<(), String> {
    let path = match account {
        Some(account) => socket::find_account(account)?,
        None => socket::pick(None)?.1,
    };
    let mut conn = Conn::open(&path)?;
    conn.send(&json!({ "op": "logs", "account": account, "follow": follow }))?;
    while let Some(message) = conn.recv() {
        match message["type"].as_str() {
            Some("log") if !all && message["line"]["source"] == "game" => {}
            Some("log") => {
                let line = &message["line"];
                println!(
                    "{}  {:<10}  {:<6}  {}",
                    output::time_of_day(line["t"].as_u64().unwrap_or(0)),
                    message["account"].as_str().unwrap_or_default(),
                    line["source"].as_str().unwrap_or_default(),
                    line["text"].as_str().unwrap_or_default()
                );
            }
            Some("error") => return Err(message["message"].as_str().unwrap_or("error").into()),
            _ => break,
        }
    }
    Ok(())
}

fn ctl(account: &str, action: Ctl) -> Result<(), String> {
    let command = match action {
        Ctl::Login => Command::Login,
        Ctl::Run { script, args } => Command::Run { script: account::script_path(&script)?, args },
        Ctl::Stop => Command::Stop,
        Ctl::Restart => Command::Restart,
        Ctl::Pause => Command::Pause,
        Ctl::Resume => Command::Resume,
        Ctl::Relogin => Command::Relogin,
        Ctl::Logout => Command::Logout,
    };
    if !socket::ctl(&socket::find_account(account)?, account, &command)? {
        println!("{account}: {}", command.unchanged());
    }
    Ok(())
}

fn check(scripts: &[PathBuf]) -> Result<(), String> {
    let mut failed = 0;
    for script in scripts {
        match aqx_script::check(script) {
            Ok(()) => println!("{}: ok", script.display()),
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", script.display());
            }
        }
    }
    match failed {
        0 => Ok(()),
        1 => Err("1 script failed to compile".into()),
        n => Err(format!("{n} scripts failed to compile")),
    }
}

fn servers(config: Option<&Path>) -> Result<(), String> {
    let api = match config::load(config) {
        Ok((config, _)) => config.defaults.api.unwrap_or_else(|| DEFAULT_API.into()),
        Err(_) if config.is_none() => DEFAULT_API.into(),
        Err(e) => return Err(e),
    };
    let list = aqx_core::servers(&api).map_err(|e| e.to_string())?;
    println!("{:<16}  {:>11}  STATE", "SERVER", "PLAYERS");
    for s in list {
        let state = match (s.online, s.members_only) {
            (false, _) => "offline",
            (true, true) => "members",
            (true, false) => "open",
        };
        println!("{:<16}  {:>11}  {state}", s.name, format!("{}/{}", s.players, s.capacity));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::fake::{FARM, Servers};

    #[test]
    fn a_session_adds_accounts_appended_to_its_config() {
        let servers = Servers::start(FARM, |_, _| {});
        let dir = std::env::temp_dir().join(format!("aqx-main-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aqx.toml");
        std::fs::write(&path, format!("[defaults]\nserver = \"Fake\"\napi = \"{}\"\n\n[[account]]\nuser = \"hero\"\npassword = \"x\"\n", servers.api)).unwrap();
        let path = std::fs::canonicalize(&path).unwrap();
        let session = {
            let path = path.clone();
            std::thread::spawn(move || {
                let (config, path) = config::load(Some(&path)).unwrap();
                start("adds-accounts", Some(&path), launches(&config, &path).unwrap(), Print::Quiet, &|_| Some(Command::Login))
            })
        };
        let accounts = || socket::session_for(&path).ok().flatten().and_then(|s| socket::accounts(&s).ok()).unwrap_or_default();
        let settle = |want: &[&str]| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while accounts() != want {
                assert!(Instant::now() < deadline, "accounts: {:?}", accounts());
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        settle(&["hero"]);

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "\n[[account]]\nname = \"renamed\"\nuser = \"HERO\"\npassword = \"x\"\n\n[[account]]\nuser = \"alt\"\npassword = \"y\"\n").unwrap();
        drop(file);
        settle(&["hero", "alt"]);

        let socket = socket::session_for(&path).unwrap().unwrap();
        let snapshots = socket::snapshot(&socket).unwrap();
        assert_eq!(snapshots[1].phase, "OFFLINE", "an added account waits to be told to log in");
        end_session(&socket).unwrap();
        let outcomes = session.join().unwrap().unwrap();
        assert!(outcomes.iter().all(|(_, r)| r.is_ok()), "{outcomes:?}");
        servers.stop();
    }
}
