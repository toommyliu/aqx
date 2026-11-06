use std::collections::{BTreeMap, HashSet};
use std::io::BufRead;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use mira_client::{
    ActionError, Aggro, Auras, BoostKind, CombatState, CounterHold, DEFAULT_RECONNECT_ATTEMPTS,
    DeviceId, Direction, Disconnect, Event, Handle, Item, ItemId, Load, MapItemId, Me, MonMapId,
    Monster, NoticeKind, Options, Pending, Phase, Place, QuestId, QuestStatus, Readiness,
    Reconnect, Respawn, Room, Server, ShopId, ShopItemId, SkillTarget, World, api,
};

const DEFAULT_PAD: &str = "Left";
const COMMANDS: &str = "join <map> [cell] [pad], cell <cell> [pad], attack <monster>, skill <slot> [monster], \
     kill <monster>, untarget, stop, cancel, who, mons [all], me, inv, drops, pick <item>, reps, quests, \
     load <quest>..., accept <quest>, complete <quest> [times] [reward], shop <id>, \
     buy <#listing> [qty], sell <item> [qty], bank, deposit <item>, withdraw <item>, \
     swap <#inventory item> <#bank item>, equip <item>, unequip <item>, use <item>, mapitem <id> [times], rest, goto <player>, walk <x> <y>, respawn spawn|back|<cell> [pad], antiafk on|off, aggro on|off|<monster>[, <monster>...], anticounter off|all|attacks, logout";

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
    /// (join <map> [cell] [pad], cell <cell> [pad], attack <monster>, skill <slot> [monster], kill <monster>, untarget, stop, cancel,
    /// who, mons [all], me, inv, drops, pick <item>, reps, quests, load <quest>..., accept <quest>,
    /// complete <quest> [times] [reward], shop <id>, buy <#listing> [qty], sell <item> [qty], bank,
    /// deposit <item>, withdraw <item>, swap <#inventory item> <#bank item>, equip <item>,
    /// unequip <item>, use <item>, mapitem <id> [times], rest, goto <player>, walk <x> <y>,
    /// respawn spawn|back|<cell> [pad], antiafk on|off, aggro on|off|<monster>[, <monster>...], anticounter off|all|attacks, logout)
    /// until it logs out. A monster or a dropped item is
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
        /// After a lost connection or a kick, log in again after this many seconds, doubling
        /// between attempts. Off unless given.
        #[arg(long, value_name = "SECS")]
        reconnect: Option<u64>,
        /// How many times in a row to try reconnecting before giving up.
        #[arg(long, value_name = "N", default_value_t = DEFAULT_RECONNECT_ATTEMPTS, requires = "reconnect")]
        reconnect_attempts: u32,
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
            reconnect,
            reconnect_attempts,
        } => {
            let reconnect = reconnect.map(|secs| Reconnect {
                attempts: reconnect_attempts,
                ..Reconnect::new(pass.clone(), Duration::from_secs(secs))
            });
            login(
                &user,
                &pass,
                &server,
                client_version,
                json,
                packets,
                reconnect,
            )
        }
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
    reconnect: Option<Reconnect>,
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
        reconnect,
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
            Err(CommandError::Action(ActionError::Closed(_)))
                if matches!(session.snapshot().phase, Phase::Closed { .. }) =>
            {
                return;
            }
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
        ["join", map, cell] => session.join_at(map, cell, DEFAULT_PAD).wait()?,
        ["join", map, cell, pad] => session.join_at(map, cell, pad).wait()?,
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
        ["untarget"] => session.untarget(),
        ["stop"] => session.exit_combat().wait()?,
        ["cancel"] => session.cancel_all(),
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
                            "turn_ins": world.turn_in_limit(quest.id).and_then(Result::ok),
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
                    let accepted = match world.turn_in_limit(quest.id) {
                        _ if !world.accepted_quests.contains(&quest.id) => String::new(),
                        Some(Ok(1)) => ", accepted, ready to turn in".into(),
                        Some(Ok(times)) => {
                            format!(", accepted, ready to turn in up to {times} at once")
                        }
                        _ => ", accepted".into(),
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
        ["shop", id] => {
            session.load_shop(ShopId(number(id)?)).wait()?;
            let world = session.snapshot();
            let Some(shop) = &world.shop else {
                return Err(CommandError::Action(ActionError::NoReply));
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&**shop).expect("shops serialize")
                );
            } else {
                println!("{} (shop {})", shop.name, shop.id);
                for item in &shop.items {
                    let currency = if item.coins { "AC" } else { "gold" };
                    println!(
                        "  #{} {} x{} for {} {currency}",
                        item.listing, item.name, item.qty, item.cost
                    );
                }
            }
        }
        ["buy", listing, rest @ ..] if rest.len() <= 1 => {
            let listing = ShopItemId(number(listing.trim_start_matches('#'))?);
            let qty = match rest.first() {
                Some(qty) => count(qty)?,
                None => session
                    .snapshot()
                    .shop
                    .as_ref()
                    .and_then(|shop| shop.items.iter().find(|i| i.listing == listing))
                    .map_or(1, |i| i.qty),
            };
            session.buy(listing, qty).wait()?;
        }
        ["sell", item @ ..] if !item.is_empty() => {
            let world = session.snapshot();
            let named = |name: &str| match name.strip_prefix('#').map(str::parse) {
                Some(Ok(id)) => Some(ItemId(id)),
                _ => world
                    .inventory
                    .iter()
                    .find(|i| i.name.eq_ignore_ascii_case(name))
                    .map(|i| i.id),
            };
            let whole = item.join(" ");
            let (id, qty) = match (named(&whole), item) {
                (Some(id), _) => (id, 1),
                (None, [name @ .., qty]) if !name.is_empty() => {
                    let name = name.join(" ");
                    let id = named(&name).ok_or_else(|| {
                        CommandError::Usage(format!("no {whole} in the inventory"))
                    })?;
                    (id, count(qty)?)
                }
                (None, _) => {
                    return Err(CommandError::Usage(format!("no {whole} in the inventory")));
                }
            };
            session.sell(id, qty).wait()?;
        }
        ["bank"] => {
            session.load_bank().wait()?;
            let world = session.snapshot();
            let bank = world.bank.as_deref().map_or(&[][..], Vec::as_slice);
            if json {
                println!("{}", serde_json::to_string(bank).expect("items serialize"));
            } else if bank.is_empty() {
                println!("the bank is empty");
            } else {
                for item in bank {
                    println!(
                        "  {} x{}/{} ({})",
                        item.name, item.qty, item.max_stack, item.kind
                    );
                }
            }
        }
        ["use", item @ ..] if !item.is_empty() => {
            let world = session.snapshot();
            let id = item_id(&item.join(" "), &world.inventory, "inventory")?;
            session.use_item(id).wait()?;
        }
        [verb @ ("equip" | "unequip"), item @ ..] if !item.is_empty() => {
            let world = session.snapshot();
            let id = item_id(&item.join(" "), &world.inventory, "inventory")?;
            let pending = if *verb == "equip" {
                session.equip(id)
            } else {
                session.unequip(id)
            };
            pending.wait()?;
        }
        ["deposit", item @ ..] if !item.is_empty() => {
            let world = session.snapshot();
            let id = item_id(&item.join(" "), &world.inventory, "inventory")?;
            session.deposit(id).wait()?;
        }
        ["withdraw", item @ ..] if !item.is_empty() => {
            session.load_bank().wait()?;
            let world = session.snapshot();
            let bank = world.bank.as_deref().map_or(&[][..], Vec::as_slice);
            let id = item_id(&item.join(" "), bank, "bank")?;
            session.withdraw(id).wait()?;
        }
        ["swap", inventory, bank] => {
            let id = |text: &str| number(text.trim_start_matches('#')).map(ItemId);
            session.swap(id(inventory)?, id(bank)?).wait()?;
        }
        ["rest"] => session.rest().wait()?,
        ["antiafk", "on"] => session.set_anti_afk(true),
        ["antiafk", "off"] => session.set_anti_afk(false),
        ["respawn", "spawn"] => session.set_respawn(Respawn::Spawn),
        ["respawn", "back"] => session.set_respawn(Respawn::WhereDied),
        ["respawn", cell] => session.set_respawn(Respawn::At {
            cell: (*cell).to_owned(),
            pad: DEFAULT_PAD.to_owned(),
        }),
        ["respawn", cell, pad] => session.set_respawn(Respawn::At {
            cell: (*cell).to_owned(),
            pad: (*pad).to_owned(),
        }),
        ["aggro", "on"] => session.set_aggro(Some(Aggro::Cell)),
        ["aggro", "off"] => session.set_aggro(None),
        ["aggro", monsters @ ..] if !monsters.is_empty() => {
            let ids = find_monsters(session, &monsters.join(" "))?;
            session.set_aggro(Some(Aggro::Only(ids)));
        }
        ["anticounter", "off"] => session.set_counter_hold(CounterHold::Off),
        ["anticounter", "all"] => session.set_counter_hold(CounterHold::All),
        ["anticounter", "attacks"] => session.set_counter_hold(CounterHold::Attacks),
        ["walk", x, y] => {
            let at = |text: &str| {
                text.parse()
                    .map_err(|_| CommandError::Usage(format!("{text:?} isn't a coordinate")))
            };
            session.walk(at(x)?, at(y)?).wait()?;
        }
        ["goto", name @ ..] if !name.is_empty() => session.goto(&name.join(" ")).wait()?,
        ["mapitem", id, rest @ ..] if rest.len() <= 1 => {
            let id = MapItemId(number(id)?);
            let times = rest.first().map_or(Ok(1), |times| count(times))?;
            for _ in 0..times {
                session.get_map_item(id).wait()?;
            }
        }
        ["me"] => {
            let world = session.snapshot();
            match &world.me {
                Some(me) if json => {
                    println!("{}", serde_json::to_string(me).expect("me serializes"))
                }
                Some(me) => print!("{}", describe_me(me, &world.readiness)),
                None => return Err(CommandError::Action(ActionError::NotReady)),
            }
        }
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

fn item_id(wanted: &str, items: &[Item], place: &str) -> Result<ItemId, CommandError> {
    if let Some(Ok(id)) = wanted.strip_prefix('#').map(str::parse) {
        return Ok(ItemId(id));
    }
    items
        .iter()
        .find(|i| i.name.eq_ignore_ascii_case(wanted))
        .map(|i| i.id)
        .ok_or_else(|| CommandError::Usage(format!("no {wanted} in the {place}")))
}

fn count(text: &str) -> Result<u32, CommandError> {
    text.parse()
        .map_err(|_| CommandError::Usage(format!("{text:?} isn't a count")))
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

fn find_monsters(session: &Handle, wanted: &str) -> Result<HashSet<MonMapId>, CommandError> {
    let world = session.snapshot();
    let Some(room) = &world.room else {
        return Err(CommandError::Action(ActionError::NotReady));
    };
    let mut ids = HashSet::new();
    for wanted in wanted.split(',').map(str::trim) {
        if let Ok(id) = wanted.trim_start_matches('#').parse() {
            ids.insert(MonMapId(id));
            continue;
        }
        let named: Vec<MonMapId> = room
            .monsters
            .iter()
            .filter(|m| m.cell == room.cell && m.alive() && m.name.eq_ignore_ascii_case(wanted))
            .map(|m| m.id)
            .collect();
        if named.is_empty() {
            return Err(CommandError::Usage(format!(
                "no live {wanted} in this cell"
            )));
        }
        ids.extend(named);
    }
    Ok(ids)
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
        let line = match world.enhancement_name(item) {
            Some(enhancement) => format!("{} [{enhancement}]\n", line.trim_end()),
            None => line,
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

fn describe_me(me: &Me, readiness: &BTreeMap<String, Readiness>) -> String {
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
        me.stats.haste
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
        let ready = match readiness.get(&skill.slot) {
            Some(Readiness::At(at)) if *at > now => {
                format!(", ready in {:.1} s", (*at - now).as_secs_f64())
            }
            Some(Readiness::Ready | Readiness::At(_)) => ", ready".into(),
            Some(Readiness::Casting) => ", casting".into(),
            Some(Readiness::Disabled) => ", disabled".into(),
            Some(Readiness::Locked) => ", locked".into(),
            Some(Readiness::NoMana) => ", no mana".into(),
            None => String::new(),
        };
        text.push_str(&format!(
            "  {:<3} {} ({} ms, {} MP, {target} x{}{ready})\n",
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
            Phase::Reconnecting {
                reason,
                attempt,
                wait_ms,
            } => format!(
                "disconnected: {reason}; reconnecting in {:.0} s (attempt {attempt})",
                Duration::from_millis(*wait_ms).as_secs_f64()
            ),
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
                Place::Bank => "the bank",
            };
            format!("{name}: {qty} in {place}")
        }
        Event::QuestAccepted { id } => format!("accepted quest {id}"),
        Event::QuestCompleted { id } => format!("completed quest {id}"),
        Event::CounterStarted { monster } => format!("monster #{monster} is countering"),
        Event::CounterEnded { monster } => format!("monster #{monster} stopped countering"),
        Event::AuraAdded { .. } | Event::AuraRemoved { .. } | Event::AwayChanged { .. } => {
            return None;
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
