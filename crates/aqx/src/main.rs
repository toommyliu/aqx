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

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use aqx_core::{CLIENT_VERSION, Control, Credentials, Recorder};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

use bot::{Backoff, Script, Spec};
use config::{BotConfig, Config};
use hub::{BotCell, Command, Hub, Print};
use socket::Conn;

const DEFAULT_API: &str = "https://game.aq.com";
const STALL_AFTER: Duration = Duration::from_secs(300);

#[derive(Parser)]
#[command(name = "aqx", version, about = "Headless AQW client")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = "Log one bot in and run a script, or stay online until Ctrl-C")]
    Run(RunArgs),
    #[command(about = "Log in every bot in the config and run its script")]
    Up(UpArgs),
    #[command(about = "Open the TUI of a running aqx process")]
    Attach {
        #[arg(help = "The process, as `aqx ps` lists it. Optional when only one runs")]
        name: Option<String>,
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
}

#[derive(Subcommand)]
enum Ctl {
    #[command(about = "Run a script, stopping the one running")]
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
struct ViewArgs {
    file: PathBuf,
    #[arg(long, value_name = "NAME", help = "Only show these commands, e.g. --cmd loginResponse --cmd joinOK")]
    cmd: Vec<String>,
    #[arg(long, help = "Don't cut lines at 160 characters")]
    full: bool,
}

#[derive(Args)]
struct RunArgs {
    #[arg(help = "Script to run once logged in. Without one, aqx stays online until Ctrl-C")]
    script: Option<PathBuf>,
    #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = parse_arg, help = "Script input read by script.args(). Repeatable")]
    args: Vec<(String, String)>,
    #[arg(short, long, value_name = "NAME", help = "The [[bot]] to use from the config")]
    account: Option<String>,
    #[arg(short, long, value_name = "FILE", help = "Config file")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "NAME", help = "Server to join, e.g. Twilly")]
    server: Option<String>,
    #[arg(long, value_name = "FILE", help = "Write every packet to FILE as JSON lines")]
    record: Option<PathBuf>,
    #[arg(long, value_name = "VERSION", default_value = CLIENT_VERSION, help = "Client version to log in with first")]
    client_version: String,
    #[arg(long, conflicts_with = "plain", help = "Print events as JSON lines")]
    json: bool,
    #[arg(long, help = "Print one line per event instead of the TUI")]
    plain: bool,
}

#[derive(Args)]
struct UpArgs {
    #[arg(short, long, value_name = "FILE", help = "Config file")]
    config: Option<PathBuf>,
    #[arg(long, help = "No TUI: print one line per event, for systemd or tmux")]
    headless: bool,
    #[arg(long, help = "Print events as JSON lines")]
    json: bool,
    #[arg(long, value_name = "NAME", help = "Name to attach to. Defaults to the config file's name")]
    name: Option<String>,
}

fn parse_arg(arg: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.into(), value.into())),
        _ => Err("expected KEY=VALUE".into()),
    }
}

fn main() -> ExitCode {
    output::start();
    let result = match Cli::parse().command {
        Cmd::Run(args) => run(args),
        Cmd::Up(args) => up(args),
        Cmd::Attach { name } => attach(name.as_deref()),
        Cmd::Ps => ps(),
        Cmd::Logs { bot, follow } => logs(bot.as_deref(), follow),
        Cmd::Ctl { bot, action } => ctl(&bot, action),
        Cmd::Servers { config } => servers(config.as_deref()),
        Cmd::View(args) => view::view(&args.file, &args.cmd, args.full),
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

fn spec(config: &Config, bot: &BotConfig, server: Option<String>, version: &str, script: Option<Script>, exit_when_done: bool) -> Spec {
    let d = &config.defaults;
    Spec {
        creds: Credentials {
            api: d.api.clone().unwrap_or_else(|| DEFAULT_API.into()),
            user: bot.user.clone(),
            password: bot.password.clone(),
        },
        server: server.or_else(|| bot.server.clone()).or_else(|| d.server.clone()),
        version: version.into(),
        script,
        room: d.room,
        restart: d.restart,
        stall_after: d.stall_after.unwrap_or(STALL_AFTER),
        on_stall: d.on_stall,
        exit_when_done,
        relogin: Backoff::new(bot::RELOGIN_FIRST, bot::RELOGIN_MAX),
        restarts: Backoff::new(bot::RESTART_FIRST, bot::RELOGIN_MAX),
    }
}

fn run(args: RunArgs) -> Result<(), String> {
    let RunArgs { script, args, account, config, server, record, client_version, json, plain } = args;
    if script.is_none() && !args.is_empty() {
        return Err("--arg needs a script".into());
    }
    let (config, path) = config::load(config.as_deref())?;
    let bot = config.bot(account.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    let recorder = record.map(|p| Recorder::create(&p).map_err(|e| format!("{}: {e}", p.display()))).transpose()?;
    let script = script.map(|path| Script { path: std::path::absolute(&path).unwrap_or(path), args });
    let launch = Launch { name: bot.name.clone(), spec: spec(&config, bot, server, &client_version, script, true), recorder };
    let print = if json {
        Print::Json
    } else if plain || !std::io::stdout().is_terminal() {
        Print::Plain
    } else {
        Print::Quiet
    };
    let results = start(&bot.name, true, vec![launch], print)?;
    results.into_iter().next().map_or(Ok(()), |(_, r)| r)
}

fn up(args: UpArgs) -> Result<(), String> {
    let (config, path) = config::load(args.config.as_deref())?;
    if config.bots.is_empty() {
        return Err(format!("{} has no [[bot]]", path.display()));
    }
    let mut launches = Vec::new();
    for bot in &config.bots {
        let script = match bot.script_path(&path) {
            Some(script) => Some(Script { path: std::path::absolute(&script).unwrap_or(script), args: bot.script_args()? }),
            None if bot.args.is_empty() => None,
            None => return Err(format!("bot {} has args but no script", bot.name)),
        };
        launches.push(Launch { name: bot.name.clone(), spec: spec(&config, bot, None, CLIENT_VERSION, script, false), recorder: None });
    }
    let name = args.name.unwrap_or_else(|| path.file_stem().map_or("aqx".into(), |s| s.to_string_lossy().into_owned()));
    let print = if args.json {
        Print::Json
    } else if args.headless || !std::io::stdout().is_terminal() {
        Print::Plain
    } else {
        Print::Quiet
    };
    let results = start(&name, false, launches, print)?;
    let failed: Vec<String> = results.into_iter().filter_map(|(bot, r)| r.err().map(|e| format!("{bot}: {e}"))).collect();
    if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }
}

type Outcomes = Vec<(String, Result<(), String>)>;

fn start(name: &str, unique: bool, launches: Vec<Launch>, print: Print) -> Result<Outcomes, String> {
    let (name, listener, _listening) = socket::bind(name, unique)?;
    let mut cells = Vec::new();
    let mut parts = Vec::new();
    for launch in launches {
        let control = Control::new().map_err(|e| e.to_string())?;
        let cell = Arc::new(BotCell::local(&launch.name, control.stop_handle()));
        cells.push(cell.clone());
        parts.push((launch, control, cell));
    }
    let hub = Arc::new(Hub::new(&name, cells, print));
    socket::serve(listener, hub.clone())?;
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
    if print == Print::Quiet {
        let fleet = tui::Fleet { title: name.clone(), cells: hub.cells.clone(), commander: tui::Commander::Local };
        let _watching = hub.watch();
        let quit_hub = hub.clone();
        tui::run(fleet, || threads.iter().all(|(_, t)| t.is_finished()), move || log_out_all(&quit_hub))?;
        if hub.cells.len() == 1 {
            let board = hub.cells[0].board();
            for line in board.log.iter().skip(board.log.len().saturating_sub(10)) {
                println!("{}  {:<6}  {}", output::clock(line.t), line.source, line.text);
            }
        }
    }
    Ok(threads.into_iter().map(|(bot, t)| (bot, t.join().unwrap_or_else(|_| Err("the bot thread crashed".into())))).collect())
}

fn log_out_all(hub: &Hub) {
    for cell in &hub.cells {
        let _ = cell.send(Command::Logout);
    }
}

fn attach(name: Option<&str>) -> Result<(), String> {
    let (name, path) = socket::pick(name)?;
    let mut conn = Conn::open(&path)?;
    conn.send(&json!({ "op": "subscribe" }))?;
    let hello = conn.recv().ok_or("the aqx process closed the connection")?;
    let bots: Vec<String> = serde_json::from_value(hello["bots"].clone()).map_err(|e| e.to_string())?;
    let cells: Vec<Arc<BotCell>> = bots.iter().map(|b| Arc::new(BotCell::mirror(b))).collect();
    let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (cells, gone) = (cells.clone(), gone.clone());
        std::thread::spawn(move || {
            while let Some(message) = conn.recv() {
                mirror(&cells, &message);
            }
            gone.store(true, Ordering::SeqCst);
        });
    }
    let fleet = tui::Fleet { title: name, cells, commander: tui::Commander::Remote(path) };
    tui::run(fleet, || gone.load(Ordering::SeqCst), || {})
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
