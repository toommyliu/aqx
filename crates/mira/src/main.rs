use std::io::BufRead;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use clap::{Parser, Subcommand};
use mira_client::{
    ActionError, CombatState, DeviceId, Direction, Disconnect, Event, Handle, Load, Me, Monster,
    NoticeKind, Options, Phase, Room, Server, SkillTarget, World, api,
};

const DEFAULT_PAD: &str = "Left";
const COMMANDS: &str = "join <map>, cell <cell> [pad], who, mons [all], me, inv, logout";

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
    /// Log in, then print what the account does and run commands read from stdin
    /// (join <map>, cell <cell> [pad], who, mons [all], me, inv, logout) until it logs out.
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
    let (session, events) = mira_client::spawn(&account, server, options)?;

    let interrupt = session.clone();
    let interrupted = AtomicBool::new(false);
    ctrlc::set_handler(move || {
        if interrupted.swap(true, Ordering::SeqCst) {
            std::process::exit(130);
        }
        interrupt.logout();
    })?;

    thread::spawn(move || serve_stdin(&session, json));

    let mut closed = None;
    for event in events {
        if let Event::Phase(Phase::Closed { reason }) = &event {
            closed = Some(reason.clone());
        }
        if json {
            println!(
                "{}",
                serde_json::to_string(&event).expect("events serialize")
            );
        } else if let Some(line) = describe(&event) {
            println!("{}{line}", stamp(started));
        }
    }
    Ok(if closed == Some(Disconnect::LoggedOut) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn serve_stdin(session: &Handle, json: bool) {
    if session.ready().wait().is_err() {
        return;
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim() == "logout" {
            return session.logout();
        }
        match run_command(session, &line, json) {
            Ok(()) => {}
            Err(CommandError::Usage(e)) => eprintln!("mira: {e}"),
            Err(CommandError::Action(ActionError::Closed(_))) => return,
            Err(CommandError::Action(e)) => eprintln!("mira: {e}"),
        }
    }
}

enum CommandError {
    Usage(String),
    Action(ActionError),
}

impl From<ActionError> for CommandError {
    fn from(e: ActionError) -> Self {
        CommandError::Action(e)
    }
}

fn run_command(session: &Handle, line: &str, json: bool) -> Result<(), CommandError> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        [] => {}
        ["join", map] => session.join(map).wait()?,
        ["cell", cell] => session.move_to_cell(cell, DEFAULT_PAD).wait()?,
        ["cell", cell, pad] => session.move_to_cell(cell, pad).wait()?,
        ["who"] => match &session.snapshot().room {
            Some(room) if json => {
                println!("{}", serde_json::to_string(room).expect("rooms serialize"))
            }
            Some(room) => print!("{}", describe_room(room)),
            None => return Err(CommandError::Action(ActionError::NotReady)),
        },
        ["mons"] => list_monsters(session, json, false)?,
        ["mons", "all"] => list_monsters(session, json, true)?,
        ["inv"] => {
            let world = session.snapshot();
            if json {
                let inv = serde_json::json!({"inventory": *world.inventory, "temp": world.temp});
                println!("{inv}");
            } else {
                print!("{}", describe_inventory(&world));
            }
        }
        ["me"] => match &session.snapshot().me {
            Some(me) if json => println!("{}", serde_json::to_string(me).expect("me serializes")),
            Some(me) => print!("{}", describe_me(me)),
            None => return Err(CommandError::Action(ActionError::NotReady)),
        },
        _ => {
            return Err(CommandError::Usage(format!(
                "can't run {line:?}; commands are {COMMANDS}"
            )));
        }
    }
    Ok(())
}

fn describe_room(room: &Room) -> String {
    let mut text = format!("{} at {}/{}\n", room.name, room.cell, room.pad);
    let mut players: Vec<_> = room.players.iter().collect();
    players.sort_by_key(|p| (p.cell.as_ref() != Some(&room.cell), p.cell.clone()));
    for player in players {
        let cell = player.cell.as_deref().unwrap_or("?");
        text.push_str(&format!("  {cell:<12} {}\n", player.name));
    }
    text
}

fn list_monsters(session: &Handle, json: bool, all: bool) -> Result<(), CommandError> {
    let world = session.snapshot();
    let Some(room) = &world.room else {
        return Err(CommandError::Action(ActionError::NotReady));
    };
    let monsters: Vec<&Monster> = room
        .monsters
        .iter()
        .filter(|m| all || m.cell == room.cell)
        .collect();
    if json {
        println!(
            "{}",
            serde_json::to_string(&monsters).expect("monsters serialize")
        );
    } else {
        print!("{}", describe_monsters(&monsters));
    }
    Ok(())
}

fn describe_inventory(world: &World) -> String {
    let mut text = String::new();
    for item in world.inventory.iter() {
        let equipped = if item.equipped { ", equipped" } else { "" };
        text.push_str(&format!(
            "  {} x{}/{} ({}{equipped})\n",
            item.name, item.qty, item.max_stack, item.kind
        ));
    }
    for item in &world.temp {
        text.push_str(&format!(
            "  {} x{}/{} (temp)\n",
            item.name, item.qty, item.max_stack
        ));
    }
    if text.is_empty() {
        text.push_str("no items\n");
    }
    text
}

fn describe_me(me: &Me) -> String {
    let mut text = format!(
        "level {}, {} gold, {}/{} HP, {}/{} MP, {}, haste {}\n",
        me.level,
        me.gold,
        me.hp,
        me.max_hp,
        me.mp,
        me.max_mp,
        state_name(me.state),
        me.haste
    );
    for skill in &me.skills {
        let target = match skill.target {
            SkillTarget::Hostile => "hostile",
            SkillTarget::Friendly => "friendly",
            SkillTarget::Myself => "self",
        };
        let locked = if skill.unlocked { "" } else { ", locked" };
        text.push_str(&format!(
            "  {:<3} {} ({} ms, {} MP, {target} x{}{locked})\n",
            skill.slot, skill.name, skill.cooldown_ms, skill.mana, skill.max_targets
        ));
    }
    text
}

fn state_name(state: CombatState) -> &'static str {
    match state {
        CombatState::Dead => "dead",
        CombatState::Idle => "idle",
        CombatState::InCombat => "in combat",
    }
}

fn describe_monsters(monsters: &[&Monster]) -> String {
    if monsters.is_empty() {
        return "no monsters\n".into();
    }
    let mut text = String::new();
    for m in monsters {
        let state = state_name(m.state);
        let aggressive = if m.aggressive { ", aggressive" } else { "" };
        text.push_str(&format!(
            "  {:<12} {} #{} {}/{} {state}{aggressive}\n",
            m.cell, m.name, m.id, m.hp, m.max_hp
        ));
    }
    text
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
        Event::Moved { cell, pad } => format!("moved to {cell}/{pad}"),
        Event::PlayerEntered { name } => format!("{name} entered"),
        Event::PlayerLeft { name } => format!("{name} left"),
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
