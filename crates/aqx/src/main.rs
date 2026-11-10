use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use aqx_client::{DeviceId, Event, Phase, Server, api};
use aqx_script::{Cancel, Level, Outcome, Output};
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod control;
mod daemon;
mod describe;
mod fleet;
mod init;
mod record;
mod runner;
mod view;
mod web;

use runner::{Job, Login, Note};

#[derive(Parser)]
#[command(
    name = "aqx",
    version,
    about = "A headless AQW client",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List the game servers and how full they are.
    Servers {
        /// Print one JSON object per server.
        #[arg(long)]
        json: bool,
    },
    /// Set up a folder for scripts: aqx.d.ts, tsconfig.json, a sample script and aqx.toml.
    Init {
        /// Where to write. Defaults to the folder of the config aqx up would read, else $AQX_HOME or ~/aqx.
        dir: Option<PathBuf>,
    },
    /// Run a session in this terminal and print its log. ctrl+c logs everyone out.
    Up {
        #[command(flatten)]
        open: commands::Open,
        /// Start the session in the background and return.
        #[arg(short, long, conflicts_with_all = ["json", "quiet"])]
        detach: bool,
        /// Print the log as JSON lines.
        #[arg(long)]
        json: bool,
        /// Print nothing; read the log with aqx logs.
        #[arg(long, conflicts_with = "json")]
        quiet: bool,
    },
    /// Serve the web UI on 127.0.0.1. It starts the session when an account logs in.
    Web {
        /// The config to read instead of ./aqx.toml or ~/aqx/aqx.toml.
        #[arg(short, long, value_name = "PATH")]
        config: Option<PathBuf>,
        #[arg(long, default_value_t = web::PORT)]
        port: u16,
        /// Start the session first if it isn't running, logging in the accounts with auto_start on.
        #[arg(long)]
        up: bool,
    },
    /// Log out every account and end the session.
    Down {
        /// The config whose session to end, instead of ./aqx.toml or ~/aqx/aqx.toml.
        #[arg(short, long, value_name = "PATH")]
        config: Option<PathBuf>,
    },
    /// List the running sessions and their accounts.
    Ps {
        /// Print one JSON object per account.
        #[arg(long)]
        json: bool,
    },
    /// Print an account's recent log.
    Logs {
        /// Only this account, by name or user.
        account: Option<String>,
        /// Keep printing new lines.
        #[arg(short, long)]
        follow: bool,
        /// Include game lines: moves, kills, drops, items and quests.
        #[arg(long)]
        all: bool,
    },
    /// Drive an account in a running session.
    Ctl {
        /// The account, by name or user.
        account: String,
        #[command(subcommand)]
        action: commands::Ctl,
    },
    /// Compile scripts and the files they import, without logging in or running them.
    Check {
        #[arg(required = true)]
        scripts: Vec<PathBuf>,
    },
    /// Download, update, and search the Skua script library in $AQX_HOME/skua or ~/aqx/skua.
    Skua {
        #[command(subcommand)]
        action: commands::Skua,
    },
    /// Log in, run a script until it ends, then log out.
    Run {
        /// The script, a .ts, .js or .cs file. With --account, defaults to the account's own script.
        script: Option<PathBuf>,
        /// The [[account]] to log in as, by name or user, instead of --user and --pass.
        #[arg(short, long, value_name = "NAME", conflicts_with_all = ["user", "pass"])]
        account: Option<String>,
        /// The config to read the account from.
        #[arg(short, long, value_name = "PATH", requires = "account")]
        config: Option<PathBuf>,
        #[arg(long, required_unless_present = "account", requires = "pass")]
        user: Option<String>,
        #[arg(long, required_unless_present = "account")]
        pass: Option<String>,
        /// Server name, as `aqx servers` lists it. With --account, defaults to the account's server.
        #[arg(long, required_unless_present = "account")]
        server: Option<String>,
        /// Log in with exactly this client version instead of following the server's minimum.
        #[arg(long, value_name = "VERSION")]
        client_version: Option<String>,
        /// A script argument. C# accepts Name=value or Group:Name=value. Repeatable.
        #[arg(long = "arg", value_name = "KEY=VALUE", value_parser = parse_arg)]
        args: Vec<(String, String)>,
        /// Write every packet sent and received to FILE as JSON lines, with the token and personal fields redacted.
        #[arg(long, value_name = "FILE")]
        record: Option<PathBuf>,
    },
}

pub(crate) fn parse_arg(text: &str) -> Result<(String, String), String> {
    text.split_once('=')
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| format!("{text} isn't KEY=VALUE"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    finish(match cli.command {
        Command::Servers { json } => servers(json),
        Command::Init { dir } => commands::init(dir),
        Command::Up {
            open, detach: true, ..
        } => commands::up_detached(&open),
        Command::Up {
            open, json, quiet, ..
        } => commands::up(&open, json, quiet),
        Command::Web { config, port, up } => commands::web(config.as_deref(), port, up),
        Command::Down { config } => commands::down(config.as_deref()),
        Command::Ps { json } => commands::ps(json),
        Command::Logs {
            account,
            follow,
            all,
        } => commands::logs(account.as_deref(), follow, all),
        Command::Ctl { account, action } => commands::ctl(&account, action),
        Command::Check { scripts } => commands::check(&scripts),
        Command::Skua { action } => commands::skua(action),
        Command::Run {
            script,
            account,
            config,
            user,
            pass,
            server,
            client_version,
            args,
            record,
        } => run_from(RunWith {
            script,
            account,
            config,
            user,
            pass,
            server,
            client_version,
            args,
            record,
        }),
    })
}

fn finish(result: anyhow::Result<ExitCode>) -> ExitCode {
    result.unwrap_or_else(|e| {
        eprintln!("aqx: {e:#}");
        ExitCode::FAILURE
    })
}

struct RunWith {
    script: Option<PathBuf>,
    account: Option<String>,
    config: Option<PathBuf>,
    user: Option<String>,
    pass: Option<String>,
    server: Option<String>,
    client_version: Option<String>,
    args: Vec<(String, String)>,
    record: Option<PathBuf>,
}

fn run_from(with: RunWith) -> anyhow::Result<ExitCode> {
    let device = DeviceId::load()?;
    let recorder = |path: &Option<PathBuf>| -> anyhow::Result<Option<record::Recorder>> {
        path.as_deref()
            .map(|path| {
                record::Recorder::create(path)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
            })
            .transpose()
    };
    let Some(name) = with.account else {
        let login = Login {
            device,
            user: with.user.expect("clap requires --user"),
            pass: with.pass.expect("clap requires --pass"),
            client_version: with.client_version,
            relogin: Some(config::Relogin {
                delay: config::RELOGIN_DELAY_DEFAULT,
                attempts: None,
            }),
            api: None,
            record: recorder(&with.record)?,
        };
        let script = with
            .script
            .ok_or_else(|| anyhow::anyhow!("name the script to run"))?;
        return run(
            &script,
            login,
            &with.server.expect("clap requires --server"),
            &with.args,
        );
    };
    let path = config::find(with.config.as_deref()).map_err(anyhow::Error::msg)?;
    let accounts = config::load(&path).map_err(anyhow::Error::msg)?;
    let account = accounts
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(&name) || a.user.eq_ignore_ascii_case(&name))
        .ok_or_else(|| anyhow::anyhow!("{}: no account {name}", path.display()))?;
    let (script, args) = match (with.script, account.script) {
        (Some(script), _) => (script, with.args),
        (None, Some(own)) => {
            let mut args = own.args;
            for (key, value) in with.args {
                args.retain(|(k, _)| *k != key);
                args.push((key, value));
            }
            (own.path, args)
        }
        (None, None) => anyhow::bail!("account {} has no script; name one to run", account.name),
    };
    let login = Login {
        device,
        user: account.user,
        pass: account.password,
        client_version: with.client_version.or(account.client_version),
        relogin: account.relogin,
        api: None,
        record: recorder(&with.record)?,
    };
    run(
        &script,
        login,
        &with.server.unwrap_or(account.server),
        &args,
    )
}

fn servers(json: bool) -> anyhow::Result<ExitCode> {
    let servers = api::servers(&DeviceId::load()?)?;
    if json {
        for server in &servers {
            println!("{}", server_json(server));
        }
        return Ok(ExitCode::SUCCESS);
    }
    let width = servers.iter().map(|s| s.name.len()).max().unwrap_or(0);
    for server in &servers {
        let mut notes = Vec::new();
        if !server.online {
            notes.push("offline");
        } else if server.full() {
            notes.push("full");
        }
        if server.members_only {
            notes.push("members");
        }
        let load = format!("{}/{}", server.players, server.capacity);
        println!("{:width$}  {load:>9}  {}", server.name, notes.join(", "))
    }
    Ok(ExitCode::SUCCESS)
}

fn server_json(server: &Server) -> serde_json::Value {
    serde_json::json!({
        "name": server.name,
        "players": server.players,
        "capacity": server.capacity,
        "online": server.online,
        "members_only": server.members_only,
    })
}

fn run(
    path: &Path,
    login: Login,
    server: &str,
    given: &[(String, String)],
) -> anyhow::Result<ExitCode> {
    let started = Instant::now();
    let cancel = Cancel::default();
    let job = Job::new(path, &cancel, move |out| show(started, &out));
    let script = job.load()?;
    let args = script.parse_args(given).map_err(anyhow::Error::msg)?;
    let login: Arc<dyn aqx_script::Account> = Arc::new(login);
    let session = login.login(server).map_err(anyhow::Error::msg)?;

    let current = Arc::new(Mutex::new(session.clone()));
    let interrupt = (cancel.clone(), Arc::clone(&current));
    let interrupted = AtomicBool::new(false);
    let skua = job.is_skua();
    ctrlc::set_handler(move || {
        if interrupted.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        interrupt.0.cancel();
        if !skua {
            interrupt
                .1
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .logout();
        }
    })?;

    let ran = runner::drive(
        &job,
        script,
        &args,
        &login,
        session,
        server.to_owned(),
        |note| match note {
            Note::Session(session) => {
                *current.lock().unwrap_or_else(PoisonError::into_inner) = session.clone();
            }
            Note::Restarting => println!("{}restarting the script", stamp(started)),
            Note::Reconnecting(why) => println!(
                "{}disconnected: {why}; the script starts again once the account is back",
                stamp(started)
            ),
        },
    );
    let closing = ran.session.subscribe();
    ran.session.logout();
    for event in closing {
        if matches!(event, Event::Phase(Phase::Closed { .. })) {
            break;
        }
    }
    Ok(match ran.outcome {
        Outcome::Finished => {
            println!("{}the script finished", stamp(started));
            ExitCode::SUCCESS
        }
        Outcome::Stopped(reason) => {
            let reason = reason.map(|r| format!(": {r}")).unwrap_or_default();
            println!("{}the script stopped{reason}", stamp(started));
            ExitCode::SUCCESS
        }
        Outcome::LoggedOut => {
            println!("{}the script logged out", stamp(started));
            ExitCode::SUCCESS
        }
        Outcome::Cancelled => {
            println!("{}the script was cancelled", stamp(started));
            ExitCode::from(130)
        }
        Outcome::Disconnected(reason) => {
            eprintln!("aqx: the session closed: {reason}");
            ExitCode::FAILURE
        }
        Outcome::Failed(why) => {
            eprintln!("aqx: {why}");
            ExitCode::FAILURE
        }
        Outcome::Restart => unreachable!("handled by the runner"),
    })
}

fn show(started: Instant, output: &Output) {
    let stamp = stamp(started);
    match output {
        Output::Log {
            level: Level::Log,
            text,
        } => println!("{stamp}{text}"),
        Output::Log {
            level: Level::Warn,
            text,
        } => println!("{stamp}warning: {text}"),
        Output::Log {
            level: Level::Error,
            text,
        } => println!("{stamp}error: {text}"),
        Output::Status(text) => println!("{stamp}status: {text}"),
    }
}

fn stamp(started: Instant) -> String {
    format!("[{:>8.3}] ", started.elapsed().as_secs_f64())
}
