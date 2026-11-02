mod config;
mod output;
mod tui;
mod view;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use aqx_core::{CLIENT_VERSION, Control, Disconnect, Halt, Login, Phase, Recorder, Server, Session, login};
use aqx_script::{Outcome, ScriptError};
use clap::{Args, Parser, Subcommand};
use output::{Mode, say};

const DEFAULT_API: &str = "https://game.aq.com";
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_EVERY: Duration = Duration::from_secs(60);
const SNAPSHOT_EVERY: Duration = Duration::from_millis(250);

#[derive(Parser)]
#[command(name = "aqx", version, about = "Headless AQW client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Log in and run a script, or stay online until Ctrl-C")]
    Run(RunArgs),
    #[command(about = "Print a --record file one packet per line")]
    View(ViewArgs),
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

fn parse_arg(arg: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.into(), value.into())),
        _ => Err("expected KEY=VALUE".into()),
    }
}

struct Script {
    path: PathBuf,
    args: Vec<(String, String)>,
}

struct Setup {
    api: String,
    user: String,
    password: String,
    server: Option<String>,
    version: String,
    config: PathBuf,
}

enum Watch {
    Status,
    Snapshots(tui::Slot),
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run(args) => run(args),
        Command::View(args) => view::view(&args.file, &args.cmd, args.full),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aqx: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: RunArgs) -> Result<(), String> {
    let RunArgs { script, args, account, config, server, record, client_version, json, plain } = args;
    if script.is_none() && !args.is_empty() {
        return Err("--arg needs a script".into());
    }
    let script = script.map(|path| Script { path, args });
    let (config, path) = config::load(config.as_deref())?;
    let bot = config.bot(account.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    let setup = Setup {
        api: config.defaults.api.clone().unwrap_or_else(|| DEFAULT_API.into()),
        user: bot.user.clone(),
        password: bot.password.clone(),
        server: server.or(config.defaults.server.clone()),
        version: client_version,
        config: path,
    };
    let control = Control::new().map_err(|e| e.to_string())?;
    let stop = control.stop_handle();
    let ctrlc_stop = stop.clone();
    ctrlc::set_handler(move || ctrlc_stop.stop()).map_err(|e| e.to_string())?;
    let recorder = record
        .map(|p| Recorder::create(&p).map_err(|e| format!("{}: {e}", p.display())))
        .transpose()?;

    if json || plain || !std::io::stdout().is_terminal() {
        output::init(if json { Mode::Json } else { Mode::Plain });
        return run_bot(setup, control, recorder, Watch::Status, script);
    }
    let (tx, rx) = mpsc::sync_channel(1024);
    output::init(Mode::Tui(tx));
    let slot: tui::Slot = Arc::new(Mutex::new(None));
    let watch = Watch::Snapshots(slot.clone());
    let bot = std::thread::Builder::new()
        .name("bot".into())
        .spawn(move || run_bot(setup, control, recorder, watch, script))
        .map_err(|e| e.to_string())?;
    tui::run(rx, slot, stop, bot)
}

fn run_bot(setup: Setup, mut control: Control, mut recorder: Option<Recorder>, watch: Watch, script: Option<Script>) -> Result<(), String> {
    let mut version = setup.version.clone();
    loop {
        say("aqx", format_args!("logging in as {} at {}, config {}", setup.user, setup.api, setup.config.display()));
        let account = login(&setup.api, &setup.user, &setup.password).map_err(|e| e.to_string())?;
        let server = pick_server(&account.servers, setup.server.as_deref(), account.member)?;
        say("aqx", format_args!("connecting to {}, client version {version}", server.name));
        let login = Login { api: &setup.api, user: &setup.user, token: &account.token, version: &version };
        let mut session =
            Session::connect(control, server, login, recorder, output::event).map_err(|e| format!("can't reach {}: {e}", server.name))?;
        watch_session(&mut session, &watch, &server.name);
        match live(&mut session, script.as_ref()) {
            Ok(result) => {
                say("aqx", "logging out");
                session.logout();
                return result;
            }
            Err(Halt::Stopped | Halt::Interrupted) => {
                say("aqx", "logging out");
                session.logout();
                return Ok(());
            }
            Err(Halt::Closed(Disconnect::ClientVersion(wanted))) if wanted != version => {
                say("aqx", format_args!("the server requires {wanted}, logging in again"));
                version = wanted;
                (control, recorder) = session.into_parts();
            }
            Err(Halt::Closed(why)) => return Err(why.to_string()),
            Err(Halt::Timeout | Halt::GoalMet) => return Err(format!("not ready after {} s, stuck in {:?}", READY_TIMEOUT.as_secs(), session.phase)),
        }
    }
}

fn live(session: &mut Session, script: Option<&Script>) -> Result<Result<(), String>, Halt> {
    session.pump_until(Some(Instant::now() + READY_TIMEOUT), |s| (s.phase == Phase::Ready).then_some(()))?;
    let Some(script) = script else { return session.pump_until(None, |_| None) };
    say("aqx", format_args!("running {}", script.path.display()));
    Ok(match aqx_script::run(session, &script.path, &script.args) {
        Ok(Outcome::Finished) => {
            say("aqx", "the script finished");
            Ok(())
        }
        Ok(Outcome::Stopped(reason)) => {
            say("aqx", format_args!("the script stopped: {}", reason.as_deref().unwrap_or("no reason given")));
            Ok(())
        }
        Err(ScriptError::Halted(halt)) => return Err(halt),
        Err(e) => Err(format!("{}: {e}", script.path.display())),
    })
}

fn watch_session(session: &mut Session, watch: &Watch, server: &str) {
    match watch {
        Watch::Status => session.watch(STATUS_EVERY, |phase, world| {
            if *phase == Phase::Ready {
                output::status(world);
            }
        }),
        Watch::Snapshots(slot) => {
            let (slot, server) = (slot.clone(), server.to_string());
            let mut published = None;
            session.watch(SNAPSHOT_EVERY, move |phase, world| {
                let version = (world.rev, phase.clone());
                if published.as_ref() != Some(&version)
                    && let Ok(mut view) = slot.lock()
                {
                    *view = Some(Arc::new(tui::View { phase: phase.clone(), server: server.clone(), world: world.clone() }));
                    published = Some(version);
                }
            });
        }
    }
}

fn pick_server<'a>(servers: &'a [Server], name: Option<&str>, member: bool) -> Result<&'a Server, String> {
    let Some(name) = name else {
        return servers
            .iter()
            .find(|s| s.online && !s.full() && (member || !s.members_only))
            .ok_or_else(|| "no server is open to this account".into());
    };
    let server = servers.iter().find(|s| s.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
        let names: Vec<_> = servers.iter().map(|s| s.name.as_str()).collect();
        format!("no server named {name}; the servers are {}", names.join(", "))
    })?;
    if !server.online {
        Err(format!("{} is offline", server.name))
    } else if server.members_only && !member {
        Err(format!("{} is for members only", server.name))
    } else {
        Ok(server)
    }
}
