mod config;
mod output;
mod tui;
mod view;

use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aqx_core::{CLIENT_VERSION, Control, Credentials, Halt, LoginError, Phase, Recorder, Session};
use aqx_script::{Outcome, ScriptError};
use clap::{Args, Parser, Subcommand};
use output::{Mode, say};

const DEFAULT_API: &str = "https://game.aq.com";
const RELOGIN_FIRST: Duration = Duration::from_secs(35);
const RELOGIN_MAX: Duration = Duration::from_secs(300);
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

fn run_bot(setup: Setup, control: Control, recorder: Option<Recorder>, watch: Watch, script: Option<Script>) -> Result<(), String> {
    let creds = Credentials { api: setup.api.clone(), user: setup.user.clone(), password: setup.password.clone() };
    let mut session = Session::new(control, creds, &setup.version, recorder, Rc::new(output::event));
    let server = Rc::new(RefCell::new(String::new()));
    watch_session(&mut session, &watch, &server);
    let mut delay = RELOGIN_FIRST;
    let mut ever_ready = false;
    loop {
        say("aqx", format_args!("logging in as {} at {}, config {}", setup.user, setup.api, setup.config.display()));
        let why = match session.login(setup.server.as_deref()) {
            Ok(()) => {
                ever_ready = true;
                server.borrow_mut().clone_from(&session.server().map(|s| s.name.clone()).unwrap_or_default());
                match live(&mut session, script.as_ref()) {
                    Ok(result) => {
                        say("aqx", "logging out");
                        session.logout();
                        return result;
                    }
                    Err(Halt::Closed(why)) if why.retryable() => {
                        if script.is_some() {
                            say("aqx", "the script starts again once the bot is back");
                        }
                        why.to_string()
                    }
                    Err(Halt::Closed(why)) => return Err(why.to_string()),
                    Err(_) => {
                        say("aqx", "logging out");
                        session.logout();
                        return Ok(());
                    }
                }
            }
            Err(LoginError::Halted(_)) => {
                session.logout();
                return Ok(());
            }
            Err(e) if ever_ready && e.retryable() => e.to_string(),
            Err(e) => return Err(e.to_string()),
        };
        if session.connected_for() >= RELOGIN_MAX {
            delay = RELOGIN_FIRST;
        }
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let wait = delay.mul_f64(0.8 + f64::from(nanos % 1000) / 1000.0 * 0.4);
        delay = (delay * 2).min(RELOGIN_MAX);
        say("aqx", format_args!("{why}; logging in again in {} s", wait.as_secs()));
        if session.wait_to_relogin(why, Instant::now() + wait).is_err() {
            session.logout();
            return Ok(());
        }
    }
}

fn live(session: &mut Session, script: Option<&Script>) -> Result<Result<(), String>, Halt> {
    let Some(script) = script else { return session.pump_until(None, |_| None) };
    say("aqx", format_args!("running {}", script.path.display()));
    Ok(match aqx_script::run(session, &script.path, &script.args, None) {
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

fn watch_session(session: &mut Session, watch: &Watch, server: &Rc<RefCell<String>>) {
    match watch {
        Watch::Status => session.watch(STATUS_EVERY, |phase, world| {
            if *phase == Phase::Ready {
                output::status(world);
            }
        }),
        Watch::Snapshots(slot) => {
            let (slot, server) = (slot.clone(), server.clone());
            let mut published = None;
            session.watch(SNAPSHOT_EVERY, move |phase, world| {
                let version = (world.rev, phase.clone());
                if published.as_ref() != Some(&version)
                    && let Ok(mut view) = slot.lock()
                {
                    *view = Some(Arc::new(tui::View { phase: phase.clone(), server: server.borrow().clone(), world: world.clone() }));
                    published = Some(version);
                }
            });
        }
    }
}
