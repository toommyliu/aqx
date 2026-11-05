use std::io::BufRead;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use clap::{Parser, Subcommand};
use mira_client::{
    ActionError, Auras, BoostKind, CombatState, DeviceId, Direction, Disconnect, Event, Handle,
    ItemId, Load, Me, MonMapId, Monster, NoticeKind, Options, Pending, Phase, Place, QuestId,
    QuestStatus, Room, Server, SkillTarget, World, api,
};

const DEFAULT_PAD: &str = "Left";
const COMMANDS: &str = "join <map>, cell <cell> [pad], attack <monster>, skill <slot> [monster], \
     kill <monster>, stop, who, mons [all], me, inv, drops, pick <item>, reps, quests, \
     load <quest>..., accept <quest>, complete <quest> [times] [reward], logout";

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
    /// (join <map>, cell <cell> [pad], attack <monster>, skill <slot> [monster], kill <monster>, stop,
    /// who, mons [all], me, inv, drops, pick <item>, reps, quests, load <quest>..., accept <quest>,
    /// complete <quest> [times] [reward], logout) until it logs out. A monster or a dropped item is
    /// its #id or its name. Quests and rewards are ids.
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
        ["attack", monster @ ..] if !monster.is_empty() => {
            let target = find_monster(session, &monster.join(" "))?;
            in_background("attack", session.attack(target));
        }
        ["kill", monster @ ..] if !monster.is_empty() => {
            let target = find_monster(session, &monster.join(" "))?;
            in_background("kill", session.kill(target));
        }
        ["skill", slot] => in_background("skill", session.use_skill(slot, None)),
        ["skill", slot, monster @ ..] => {
            let target = find_monster(session, &monster.join(" "))?;
            in_background("skill", session.use_skill(slot, Some(target)));
        }
        ["stop"] => session.exit_combat().wait()?,
        ["load", ids @ ..] if !ids.is_empty() => {
            let ids = ids.iter().map(|id| number(id).map(QuestId));
            session
                .load_quests(&ids.collect::<Result<Vec<_>, _>>()?)
                .wait()?;
        }
        ["accept", id] => session.accept_quest(QuestId(number(id)?)).wait()?,
        ["complete", id, rest @ ..] if rest.len() <= 2 => {
            let times = rest.first().map_or(Ok(1), |times| number(times))?;
            let reward = rest.get(1).map(|id| number(id).map(ItemId)).transpose()?;
            let times = u32::try_from(times)
                .map_err(|_| CommandError::Usage(format!("{times} isn't a turn-in count")))?;
            session
                .complete_quest(QuestId(number(id)?), reward, times)
                .wait()?;
        }
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
        ["quests"] => {
            let world = session.snapshot();
            if json {
                let quests: Vec<_> = world
                    .quests
                    .values()
                    .map(|quest| {
                        serde_json::json!({
                            "quest": quest,
                            "status": world.quest_status(quest.id),
                            "accepted": world.accepted_quests.contains(&quest.id),
                        })
                    })
                    .collect();
                println!("{}", serde_json::Value::Array(quests));
            } else if world.quests.is_empty() {
                println!("no quests loaded");
            } else {
                for quest in world.quests.values() {
                    let status = match world.quest_status(quest.id) {
                        Some(QuestStatus::Done) => "done",
                        Some(QuestStatus::DoneThisPeriod) => "done for now",
                        Some(QuestStatus::Locked) => "locked",
                        Some(QuestStatus::Available) => "available",
                        None => "unknown",
                    };
                    let accepted = if world.accepted_quests.contains(&quest.id) {
                        ", accepted"
                    } else {
                        ""
                    };
                    println!("  {} {} ({status}{accepted})", quest.id, quest.name);
                }
            }
        }
        ["reps"] => {
            let world = session.snapshot();
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&world.factions).expect("factions serialize")
                );
            } else if world.factions.is_empty() {
                println!("no factions");
            } else {
                for faction in &world.factions {
                    println!(
                        "  {} rank {} ({} rep)",
                        faction.name, faction.rank, faction.rep
                    );
                }
            }
        }
        ["drops"] => {
            let world = session.snapshot();
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&world.drops).expect("drops serialize")
                );
            } else if world.drops.is_empty() {
                println!("no drops");
            } else {
                for drop in &world.drops {
                    println!("  #{} {} x{}", drop.id, drop.name, drop.qty);
                }
            }
        }
        ["pick", item @ ..] if !item.is_empty() => {
            let wanted = item.join(" ");
            let id = match wanted.trim_start_matches('#').parse() {
                Ok(id) => ItemId(id),
                Err(_) => session
                    .snapshot()
                    .drops
                    .iter()
                    .find(|d| d.name.eq_ignore_ascii_case(&wanted))
                    .map(|d| d.id)
                    .ok_or_else(|| CommandError::Usage(format!("no {wanted} on offer")))?,
            };
            session.pick_drop(id).wait()?;
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

fn number(text: &str) -> Result<i64, CommandError> {
    text.parse()
        .map_err(|_| CommandError::Usage(format!("{text:?} isn't a number")))
}

fn in_background(what: &'static str, pending: Pending) {
    thread::spawn(move || {
        if let Err(e) = pending.wait() {
            eprintln!("mira: {what}: {e}");
        }
    });
}

fn find_monster(session: &Handle, wanted: &str) -> Result<MonMapId, CommandError> {
    if let Ok(id) = wanted.trim_start_matches('#').parse() {
        return Ok(MonMapId(id));
    }
    let world = session.snapshot();
    let Some(room) = &world.room else {
        return Err(CommandError::Action(ActionError::NotReady));
    };
    room.monsters
        .iter()
        .filter(|m| m.cell == room.cell && m.alive() && m.name.eq_ignore_ascii_case(wanted))
        .min_by_key(|m| m.hp)
        .map(|m| m.id)
        .ok_or_else(|| CommandError::Usage(format!("no live {wanted} in this cell")))
}

fn describe_room(room: &Room) -> String {
    let mut text = format!("{} at {}/{}\n", room.name, room.cell, room.pad);
    let mut players: Vec<_> = room.players.iter().collect();
    players.sort_by_key(|p| (p.cell.as_ref() != Some(&room.cell), p.cell.clone()));
    for player in players {
        let cell = player.cell.as_deref().unwrap_or("?");
        let level = player
            .level
            .map_or_else(|| "?".to_owned(), |l| l.to_string());
        let class = player.class_name.as_deref().unwrap_or("?");
        text.push_str(&format!(
            "  {cell:<12} {} (level {level} {class}, {}/{} HP, {})\n",
            player.name,
            player.hp,
            player.max_hp,
            state_name(player.state)
        ));
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
        let line = match item.class_rank() {
            Some(rank) => format!(
                "  {} (class, rank {rank}, {} CP{equipped})\n",
                item.name, item.qty
            ),
            None => format!(
                "  {} x{}/{} ({}{equipped})\n",
                item.name, item.qty, item.max_stack, item.kind
            ),
        };
        text.push_str(&line);
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
        "level {} ({}/{} xp), {} rank {} ({} CP), {} gold, {} AC, {}/{} HP, {}/{} MP, {}, haste {}\n",
        me.level,
        me.exp,
        me.exp_to_level,
        me.class_name,
        me.class_rank(),
        me.class_points,
        me.gold,
        me.coins,
        me.hp,
        me.max_hp,
        me.mp,
        me.max_mp,
        state_name(me.state),
        me.haste
    );
    let now = std::time::Instant::now();
    let boosts: Vec<String> = [
        (BoostKind::Xp, "xp"),
        (BoostKind::Gold, "gold"),
        (BoostKind::Rep, "rep"),
        (BoostKind::ClassPoints, "class points"),
    ]
    .into_iter()
    .filter_map(|(kind, name)| {
        let left = me.boosts.remaining(kind, now).as_secs();
        (left > 0).then(|| format!("{name} {}m", left.div_ceil(60)))
    })
    .collect();
    if !boosts.is_empty() {
        text.push_str(&format!("  boosts: {}\n", boosts.join(", ")));
    }
    let auras = aura_names(&me.auras);
    if !auras.is_empty() {
        text.push_str(&format!("  auras{auras}\n"));
    }
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

fn aura_names(auras: &Auras) -> String {
    let names: Vec<&str> = auras
        .active
        .iter()
        .chain(&auras.passive)
        .map(|aura| aura.name.as_str())
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!(": {}", names.join(", "))
    }
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
        let auras = aura_names(&m.auras);
        text.push_str(&format!(
            "  {:<12} {} #{} {}/{} {state}{aggressive}{auras}\n",
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
        Event::Killed {
            name, gold, exp, ..
        } => format!("killed {name}, +{gold} gold, +{exp} xp"),
        Event::Died => "died".into(),
        Event::Respawned => "respawned".into(),
        Event::LeveledUp { level } => format!("reached level {level}"),
        Event::DropOffered { name, qty, .. } => format!("drop offered: {name} x{qty}"),
        Event::ItemChanged {
            name, qty, place, ..
        } => {
            let place = match place {
                Place::Inventory => "inventory",
                Place::Temp => "temp",
            };
            format!("{name}: {qty} in {place}")
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
