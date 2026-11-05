use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use clap::{Parser, Subcommand};
use mira_client::{
    DeviceId, Direction, Disconnect, Event, Load, NoticeKind, Options, Phase, Server, Session, api,
};

#[derive(Parser)]
#[command(name = "mira", version, about = "A headless AQW client")]
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
    /// Log in, then print what the account does until Ctrl-C logs it out.
    Login {
        #[arg(long)]
        user: String,
        #[arg(long)]
        pass: String,
        /// Server name, as `mira servers` lists it.
        #[arg(long)]
        server: String,
        /// Log in with exactly this client version instead of following the server's minimum.
        #[arg(long, value_name = "VERSION")]
        client_version: Option<String>,
        /// Print one JSON object per event.
        #[arg(long)]
        json: bool,
        /// Also print every packet sent and received, with the token and personal fields redacted.
        #[arg(long)]
        packets: bool,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Servers { json } => servers(json),
        Command::Login {
            user,
            pass,
            server,
            client_version,
            json,
            packets,
        } => login(&user, &pass, &server, client_version, json, packets),
    };
    result.unwrap_or_else(|e| {
        eprintln!("mira: {e:#}");
        ExitCode::FAILURE
    })
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

fn login(
    user: &str,
    pass: &str,
    server: &str,
    client_version: Option<String>,
    json: bool,
    packets: bool,
) -> anyhow::Result<ExitCode> {
    let started = Instant::now();
    if !json {
        println!("{}requesting a login token", stamp(started));
    }
    let account = api::login(&DeviceId::load()?, user, pass)?;
    let server = account.pick_server(server)?.clone();
    let options = Options {
        client_version,
        trace_packets: packets,
    };
    let mut session = Session::new(&account, options, move |event| {
        if json {
            println!(
                "{}",
                serde_json::to_string(event).expect("events serialize")
            );
        } else if let Some(line) = describe(event) {
            println!("{}{line}", stamp(started));
        }
    })?;

    let control = session.control();
    let interrupted = AtomicBool::new(false);
    ctrlc::set_handler(move || {
        if interrupted.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        control.logout();
    })?;

    let why = match session.login(&server) {
        Ok(()) => session.run(),
        Err(_) => match session.phase() {
            Phase::Closed { reason } => reason.clone(),
            _ => unreachable!("a failed login always closes the session"),
        },
    };
    Ok(if why == Disconnect::LoggedOut {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn stamp(started: Instant) -> String {
    format!("[{:>8.3}] ", started.elapsed().as_secs_f64())
}

fn describe(event: &Event) -> Option<String> {
    Some(match event {
        Event::Phase(phase) => match phase {
            Phase::Connecting { server, version } => {
                format!("connecting to {server} as client {version}")
            }
            Phase::Handshaking => "handshaking".into(),
            Phase::LoggingIn => "logging in".into(),
            Phase::Loading {
                step: Load::Joining,
            } => "joining the first map".into(),
            Phase::Loading {
                step: Load::Character,
            } => "loading the character".into(),
            Phase::Loading {
                step: Load::Inventory,
            } => "loading the inventory".into(),
            Phase::Ready => return None,
            Phase::Closed { reason } => format!("disconnected: {reason}"),
        },
        Event::Joined {
            map,
            cell,
            pad,
            players,
        } => {
            format!("joined {map} at {cell}/{pad}, {players} other players")
        }
        Event::Ready { level, gold, items } => {
            format!("ready: level {level}, {gold} gold, {items} items")
        }
        Event::Chat {
            channel,
            sender,
            text,
        } => format!("[{channel}] {sender}: {text}"),
        Event::Whisper { sender, text } => format!("[whisper] {sender}: {text}"),
        Event::Notice { kind, text } => {
            let kind = match kind {
                NoticeKind::Server => "server",
                NoticeKind::Notify => "notice",
                NoticeKind::Warning => "warning",
            };
            format!("{kind}: {text}")
        }
        Event::Unreadable { error } => format!("unreadable message: {error}"),
        Event::Packet { direction, text } => {
            let arrow = match direction {
                Direction::In => "<",
                Direction::Out => ">",
            };
            format!("{arrow} {text}")
        }
    })
}
