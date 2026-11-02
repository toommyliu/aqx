mod bot;
mod config;
#[cfg(test)]
#[path = "../../aqx-core/tests/server/mod.rs"]
mod fake;
mod hub;
mod output;
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

use bot::{Backoff, Script, Spec};
use config::{BotConfig, Config, Settings};
use hub::{BotCell, Command, Hub, Print};
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
    #[command(about = "Log one bot in and print what it does until its script ends, or until Ctrl-C without one")]
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
    #[command(about = "Log out every bot and end the session")]
    Down {
        #[arg(short, long, value_name = "FILE", help = "Config file")]
        config: Option<PathBuf>,
    },
    #[command(about = "List running aqx processes and their bots")]
    Ps,
    #[command(about = "Print a bot's recent log")]
    Logs {
        #[arg(help = "Only this bot")]
        bot: Option<String>,
        #[arg(short, long, help = "Keep printing new lines")]
        follow: bool,
    },
    #[command(about = "Control a bot in a running aqx process")]
    Ctl {
        bot: String,
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
}

#[derive(Subcommand)]
enum Ctl {
    #[command(about = "Log in and run the bot's own script")]
    Login,
    #[command(about = "Run a script, stopping the one running. Logs the bot in if it's offline")]
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
    #[arg(value_name = "BOT", help = "Bots to log in, by name or user. Without any, a new session logs in every bot")]
    bots: Vec<String>,
    #[arg(short, long, value_name = "FILE", help = "Config file")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "FILE", help = "Run this script on the bots instead of their own")]
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
    #[arg(help = "Script to run instead of the bot's own. A bot without one stays online until Ctrl-C")]
    script: Option<PathBuf>,
    #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = parse_arg, help = "Script input read by script.args(), over the bot's args. Repeatable")]
    args: Vec<(String, String)>,
    #[arg(long, conflicts_with_all = ["script", "args"], help = "Log in without running the bot's script")]
    idle: bool,
    #[arg(short, long, value_name = "NAME", help = "The [[bot]] to use from the config, by name or user")]
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
    output::start();
    let cli = Cli::parse();
    let result = match cli.command {
        None => open(&cli.open),
        Some(Cmd::Run(args)) => run(args),
        Some(Cmd::Serve { wanted, json, quiet }) => serve(&wanted, json, quiet),
        Some(Cmd::Down { config }) => down(config.as_deref()),
        Some(Cmd::Ps) => ps(),
        Some(Cmd::Logs { bot, follow }) => logs(bot.as_deref(), follow),
        Some(Cmd::Ctl { bot, action }) => ctl(&bot, action),
        Some(Cmd::Servers { config }) => servers(config.as_deref()),
        Some(Cmd::View(args)) => view::view(&args.file, &args.cmd, args.full),
        Some(Cmd::Schema) => {
            println!("{}", config::schema());
            Ok(())
        }
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

fn spec(bot: &BotConfig, settings: Settings, version: &str, script: Option<Script>, exit_when_done: bool) -> Spec {
    Spec {
        creds: Credentials { api: settings.api.unwrap_or_else(|| DEFAULT_API.into()), user: bot.user.clone(), password: bot.password.clone() },
        server: settings.server,
        version: version.into(),
        script,
        room: settings.room,
        restart: settings.restart.unwrap_or_default(),
        stall_after: settings.stall_after.unwrap_or(STALL_AFTER),
        on_stall: settings.on_stall.unwrap_or_default(),
        exit_when_done,
        relogin: Backoff::new(bot::RELOGIN_FIRST, bot::RELOGIN_MAX),
        restarts: Backoff::new(bot::RESTART_FIRST, bot::RELOGIN_MAX),
    }
}

fn run(args: RunArgs) -> Result<(), String> {
    let RunArgs { script, args, idle, account, config, server, record, client_version, json } = args;
    let (config, path) = config::load(config.as_deref())?;
    let bot = config.bot(account.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    let script = bot.run_script(&path, script, args, idle)?;
    let recorder = record.map(|p| Recorder::create(&p).map_err(|e| format!("{}: {e}", p.display()))).transpose()?;
    let mut settings = bot.settings(&config.defaults);
    settings.server = server.or(settings.server);
    let exit_when_done = script.is_some();
    let launch = Launch { name: bot.name().into(), spec: spec(bot, settings, &client_version, script, exit_when_done), recorder };
    let print = if json { Print::Json } else { Print::Plain };
    let results = start(bot.name(), None, vec![launch], print, &|_| Some(Command::Login))?;
    results.into_iter().next().map_or(Ok(()), |(_, r)| r)
}

fn open(wanted: &Wanted) -> Result<(), String> {
    if !std::io::stdout().is_terminal() {
        return Err("aqx opens a TUI and needs a terminal; use aqx serve or aqx run".into());
    }
    let (config, path) = config::load(wanted.config.as_deref())?;
    let bots = named(&config, &wanted.bots)?;
    let command = command(wanted)?;
    let session = match socket::session_for(&path)? {
        Some(session) => {
            if bots.is_empty() && wanted.script.is_some() {
                return Err("aqx is already running for this config; name the bots to run the script on".into());
            }
            for bot in &bots {
                socket::ctl(&session, bot, &command)?;
            }
            session
        }
        None => start_session(&path, &bots, &command)?,
    };
    attach(&session)
}

fn named(config: &Config, names: &[String]) -> Result<Vec<String>, String> {
    let mut bots: Vec<String> = Vec::new();
    for name in names {
        let bot = config.bot(Some(name))?.name().to_string();
        if !bots.contains(&bot) {
            bots.push(bot);
        }
    }
    Ok(bots)
}

fn command(wanted: &Wanted) -> Result<Command, String> {
    let Some(script) = &wanted.script else { return Ok(Command::Login) };
    let script = std::path::absolute(script).map_err(|e| format!("{}: {e}", script.display()))?;
    if !script.is_file() {
        return Err(format!("{} isn't a file", script.display()));
    }
    Ok(Command::Run { script, args: wanted.args.clone() })
}

fn start_session(config: &Path, bots: &[String], command: &Command) -> Result<PathBuf, String> {
    let stem = config.file_stem().map_or("aqx".into(), |s| s.to_string_lossy().into_owned());
    let mut hasher = std::hash::DefaultHasher::new();
    config.hash(&mut hasher);
    let log = socket::prepare_dir()?.join(format!("{stem}-{:016x}.log", hasher.finish()));
    let errors = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut serve = std::process::Command::new(exe);
    serve.args(["serve", "--quiet", "--config"]).arg(config).args(bots);
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
    if config.bots.is_empty() {
        return Err(format!("{} has no [[bot]]", path.display()));
    }
    if let Some(session) = socket::session_for(&path)? {
        return Err(format!("aqx is already running for {}; its socket is {}", path.display(), session.display()));
    }
    let bots = named(&config, &wanted.bots)?;
    let command = command(wanted)?;
    let mut launches = Vec::new();
    for bot in &config.bots {
        let spec = spec(bot, bot.settings(&config.defaults), CLIENT_VERSION, bot.script(&path)?, false);
        launches.push(Launch { name: bot.name().into(), spec, recorder: None });
    }
    let name = path.file_stem().map_or("aqx".into(), |s| s.to_string_lossy().into_owned());
    let print = match (json, quiet) {
        (true, _) => Print::Json,
        (_, true) => Print::Quiet,
        _ => Print::Plain,
    };
    let first = |bot: &str| (bots.is_empty() || bots.iter().any(|b| b == bot)).then(|| command.clone());
    let results = start(&name, Some(&path), launches, print, &first)?;
    let failed: Vec<String> = results.into_iter().filter_map(|(bot, r)| r.err().map(|e| format!("{bot}: {e}"))).collect();
    if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }
}

fn down(config: Option<&Path>) -> Result<(), String> {
    let (_, path) = config::load(config)?;
    let session = socket::session_for(&path)?.ok_or_else(|| format!("aqx isn't running for {}", path.display()))?;
    for bot in socket::bots(&session)? {
        socket::ctl(&session, &bot, &Command::Logout)?;
    }
    let deadline = Instant::now() + DOWN_TIMEOUT;
    while std::os::unix::net::UnixStream::connect(&session).is_ok() {
        if Instant::now() >= deadline {
            return Err(format!("the session is still logging out after {}s", DOWN_TIMEOUT.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

type Outcomes = Vec<(String, Result<(), String>)>;

fn start(name: &str, config: Option<&Path>, launches: Vec<Launch>, print: Print, first: &dyn Fn(&str) -> Option<Command>) -> Result<Outcomes, String> {
    let (name, listener, _listening) = socket::bind(name, true)?;
    let mut cells = Vec::new();
    let mut parts = Vec::new();
    for launch in launches {
        let control = Control::new().map_err(|e| e.to_string())?;
        let cell = Arc::new(BotCell::local(&launch.name, control.stop_handle()));
        if let Some(command) = first(&launch.name) {
            cell.send(command)?;
        }
        cells.push(cell.clone());
        parts.push((launch, control, cell));
    }
    let hub = Arc::new(Hub::new(&name, cells, print));
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
    let threads: Vec<(String, JoinHandle<Result<(), String>>)> = parts
        .into_iter()
        .map(|(launch, control, cell)| {
            let hub = hub.clone();
            let thread = std::thread::Builder::new()
                .name(launch.name.clone())
                .spawn(move || bot::run(launch.spec, launch.recorder, control, cell, hub))
                .map_err(|e| e.to_string())?;
            Ok((launch.name, thread))
        })
        .collect::<Result<_, String>>()?;
    while !hub.cells.iter().all(|c| c.offline()) {
        std::thread::sleep(Duration::from_millis(250));
    }
    for cell in &hub.cells {
        cell.close();
    }
    Ok(threads.into_iter().map(|(bot, t)| (bot, t.join().unwrap_or_else(|_| Err("the bot thread crashed".into())))).collect())
}

fn log_out_all(hub: &Hub) {
    for cell in &hub.cells {
        let _ = cell.send(Command::Logout);
    }
}

fn attach(path: &Path) -> Result<(), String> {
    let mut conn = Conn::open(path)?;
    conn.send(&json!({ "op": "subscribe" }))?;
    let hello = conn.recv().ok_or("the aqx process closed the connection")?;
    let name = hello["name"].as_str().unwrap_or("aqx").to_string();
    let bots: Vec<String> = serde_json::from_value(hello["bots"].clone()).map_err(|e| e.to_string())?;
    let cells: Vec<Arc<BotCell>> = bots.iter().map(|b| Arc::new(BotCell::mirror(b))).collect();
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
        && let [only] = &fleet.cells[..]
    {
        let board = only.board();
        for line in board.log.iter().skip(board.log.len().saturating_sub(10)) {
            println!("{}  {:<6}  {}", output::clock(line.t), line.source, line.text);
        }
    }
    Ok(())
}

fn mirror(cells: &[Arc<BotCell>], message: &serde_json::Value) {
    match message["type"].as_str() {
        Some("log") => {
            let (Some(bot), Ok(line)) = (message["bot"].as_str(), serde_json::from_value::<hub::LogLine>(message["line"].clone())) else { return };
            if let Some(cell) = cells.iter().find(|c| c.name == bot) {
                let mut board = cell.board();
                if board.log.len() == hub::LOG_LINES {
                    board.log.pop_front();
                }
                board.log.push_back(line);
            }
        }
        Some("snapshot") => {
            let Ok(snapshot) = serde_json::from_value::<hub::Snapshot>(message["snapshot"].clone()) else { return };
            if let Some(cell) = cells.iter().find(|c| c.name == snapshot.bot) {
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
    println!("{:<12}  {:<10}  {:<12}  {:<22}  {:<24}  {:<6}  STATUS", "PROCESS", "BOT", "STATE", "SCRIPT", "WHERE", "UP");
    for (name, path) in all {
        let bots = match socket::snapshot(&path) {
            Ok(bots) => bots,
            Err(e) => {
                println!("{name:<12}  {e}");
                continue;
            }
        };
        for s in bots {
            let (state, script) = (s.state(), s.script_label());
            let place = s.game.as_ref().map_or(String::new(), |g| format!("{} {}", g.area, g.cell));
            let status = if s.phase != "READY" && !s.detail.is_empty() { &s.detail } else { &s.status };
            println!("{name:<12}  {:<10}  {state:<12}  {script:<22}  {place:<24}  {:<6}  {status}", s.bot, output::span(s.up_secs));
        }
    }
    Ok(())
}

fn logs(bot: Option<&str>, follow: bool) -> Result<(), String> {
    let path = match bot {
        Some(bot) => socket::find_bot(bot)?,
        None => socket::pick(None)?.1,
    };
    let mut conn = Conn::open(&path)?;
    conn.send(&json!({ "op": "logs", "bot": bot, "follow": follow }))?;
    while let Some(message) = conn.recv() {
        match message["type"].as_str() {
            Some("log") => {
                let line = &message["line"];
                println!(
                    "{}  {:<10}  {:<6}  {}",
                    output::clock(line["t"].as_u64().unwrap_or(0)),
                    message["bot"].as_str().unwrap_or_default(),
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

fn ctl(bot: &str, action: Ctl) -> Result<(), String> {
    let command = match action {
        Ctl::Login => Command::Login,
        Ctl::Run { script, args } => {
            let script = std::path::absolute(&script).map_err(|e| format!("{}: {e}", script.display()))?;
            if !script.is_file() {
                return Err(format!("{} isn't a file", script.display()));
            }
            Command::Run { script, args }
        }
        Ctl::Stop => Command::Stop,
        Ctl::Restart => Command::Restart,
        Ctl::Pause => Command::Pause,
        Ctl::Resume => Command::Resume,
        Ctl::Relogin => Command::Relogin,
        Ctl::Logout => Command::Logout,
    };
    socket::ctl(&socket::find_bot(bot)?, bot, &command)
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
