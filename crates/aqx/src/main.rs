mod config;
mod output;
mod view;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use aqx_core::{CLIENT_VERSION, Control, Disconnect, Halt, Login, Phase, Recorder, Server, Session, login};
use clap::{Args, Parser, Subcommand};
use output::say;

const DEFAULT_API: &str = "https://game.aq.com";
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_EVERY: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[command(name = "aqx", version, about = "Headless AQW client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Log in and stay online until Ctrl-C")]
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
    #[arg(help = "Script to run. Not supported yet")]
    script: Option<PathBuf>,
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
    #[arg(long, help = "Print events as JSON lines")]
    json: bool,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run(args) => {
            output::init(args.json);
            run(args)
        }
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
    let RunArgs { script, account, config, server, record, client_version: mut version, .. } = args;
    if let Some(script) = script {
        return Err(format!("can't run {}: scripts aren't supported yet. Run without one to log in", script.display()));
    }
    let (config, path) = config::load(config.as_deref())?;
    let bot = config.bot(account.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    let api = config.defaults.api.as_deref().unwrap_or(DEFAULT_API);
    let server_name = server.or(config.defaults.server.clone());

    let mut control = Control::new().map_err(|e| e.to_string())?;
    let stop = control.stop_handle();
    ctrlc::set_handler(move || stop.stop()).map_err(|e| e.to_string())?;
    let mut recorder = record
        .map(|p| Recorder::create(&p).map_err(|e| format!("{}: {e}", p.display())))
        .transpose()?;

    loop {
        say("aqx", format_args!("logging in as {} at {api}, config {}", bot.user, path.display()));
        let account = login(api, &bot.user, &bot.password).map_err(|e| e.to_string())?;
        let server = pick_server(&account.servers, server_name.as_deref(), account.member)?;
        say("aqx", format_args!("connecting to {}, client version {version}", server.name));
        let login = Login { user: &bot.user, token: &account.token, version: &version };
        let mut session =
            Session::connect(control, server, login, recorder, output::event).map_err(|e| format!("can't reach {}: {e}", server.name))?;
        let outcome = session
            .pump_until(Some(Instant::now() + READY_TIMEOUT), |s| (s.phase == Phase::Ready).then_some(()))
            .and_then(|()| stay_online(&mut session));
        match outcome {
            Ok(()) => return Ok(()),
            Err(Halt::Stopped) => {
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
            Err(Halt::Timeout) => return Err(format!("not ready after {} s, stuck in {:?}", READY_TIMEOUT.as_secs(), session.phase)),
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

fn stay_online(session: &mut Session) -> Result<(), Halt> {
    loop {
        match session.pump_until(Some(Instant::now() + STATUS_EVERY), |_| None::<()>) {
            Err(Halt::Timeout) => output::status(&session.world),
            other => return other,
        }
    }
}
