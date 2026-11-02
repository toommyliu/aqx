mod config;
mod output;
mod tui;
mod view;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use aqx_core::{ActError, CLIENT_VERSION, Control, Disconnect, Fight, Halt, ItemRef, Login, Phase, Recorder, Server, Session, login};
use aqx_protocol::{ItemId, QuestId, ShopId};
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
    #[arg(long, conflicts_with = "plain", help = "Print events as JSON lines")]
    json: bool,
    #[arg(long, help = "Print one line per event instead of the TUI")]
    plain: bool,
    #[arg(
        long = "do",
        value_name = "STEP",
        value_parser = parse_task,
        help = "Run a step once ready: \"join MAP [CELL PAD]\", \"jump CELL [PAD]\", \"kill NAME [COUNT]\", \"farm ITEM COUNT [from NAME]\", \"accept QUEST\", \"complete QUEST [REWARD]\", \"withdraw ITEM\", \"deposit ITEM\", \"buy SHOP ITEM [QTY]\", \"sell ITEM [QTY]\", \"equip ITEM\", \"pickup ITEM\" or \"rest\". Repeatable"
    )]
    tasks: Vec<Task>,
}

#[derive(Clone, Debug)]
enum Task {
    Join { map: String, cell: Option<(String, String)> },
    Jump { cell: String, pad: String },
    Kill { name: String, count: u32 },
    Farm { item: String, count: u32, from: Option<String> },
    Accept(u32),
    Complete { quest: u32, reward: Option<u32> },
    Withdraw(String),
    Deposit(String),
    Buy { shop: u32, item: String, qty: u32 },
    Sell { item: String, qty: u32 },
    Equip(String),
    Pickup(String),
    Rest,
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Join { map, cell: None } => write!(f, "join {map}"),
            Task::Join { map, cell: Some((cell, pad)) } => write!(f, "join {map} {cell} {pad}"),
            Task::Jump { cell, pad } => write!(f, "jump {cell} {pad}"),
            Task::Kill { name, count: 1 } => write!(f, "kill {name}"),
            Task::Kill { name, count } => write!(f, "kill {name} {count}"),
            Task::Farm { item, count, from: None } => write!(f, "farm {item} {count}"),
            Task::Farm { item, count, from: Some(monster) } => write!(f, "farm {item} {count} from {monster}"),
            Task::Accept(quest) => write!(f, "accept {quest}"),
            Task::Complete { quest, reward: None } => write!(f, "complete {quest}"),
            Task::Complete { quest, reward: Some(item) } => write!(f, "complete {quest} {item}"),
            Task::Withdraw(item) => write!(f, "withdraw {item}"),
            Task::Deposit(item) => write!(f, "deposit {item}"),
            Task::Buy { shop, item, qty } => write!(f, "buy {shop} {item} {qty}"),
            Task::Sell { item, qty } => write!(f, "sell {item} {qty}"),
            Task::Equip(item) => write!(f, "equip {item}"),
            Task::Pickup(item) => write!(f, "pickup {item}"),
            Task::Rest => write!(f, "rest"),
        }
    }
}

fn name_and_count(words: &[&str]) -> Result<(String, u32), String> {
    match words.split_last().map(|(last, name)| (last.parse::<u32>(), name)) {
        Some((Ok(0), _)) => Err("the count must be at least 1".into()),
        Some((Ok(count), name)) if !name.is_empty() => Ok((name.join(" "), count)),
        _ => Ok((words.join(" "), 1)),
    }
}

fn parse_task(step: &str) -> Result<Task, String> {
    if let Some(rest) = step.strip_prefix("farm ") {
        let (goal, from) = match rest.split_once(" from ") {
            Some((goal, monster)) => (goal, Some(monster.trim().to_string())),
            None => (rest, None),
        };
        let words: Vec<&str> = goal.split_whitespace().collect();
        return match words.split_last().map(|(last, item)| (last.parse::<u32>(), item)) {
            Some((Ok(count), item)) if count > 0 && !item.is_empty() => Ok(Task::Farm { item: item.join(" "), count, from }),
            _ => Err("expected farm ITEM COUNT [from NAME], with COUNT at least 1".into()),
        };
    }
    let words: Vec<&str> = step.split_whitespace().collect();
    let text = |ws: &[&str]| ws.join(" ");
    Ok(match words.as_slice() {
        ["join", map] => Task::Join { map: map.to_string(), cell: None },
        ["join", map, cell, pad] => Task::Join { map: map.to_string(), cell: Some((cell.to_string(), pad.to_string())) },
        ["jump", cell] => Task::Jump { cell: cell.to_string(), pad: "Left".into() },
        ["jump", cell, pad] => Task::Jump { cell: cell.to_string(), pad: pad.to_string() },
        ["kill", rest @ ..] if !rest.is_empty() => {
            let (name, count) = name_and_count(rest)?;
            Task::Kill { name, count }
        }
        ["pickup", item @ ..] if !item.is_empty() => Task::Pickup(text(item)),
        ["withdraw", item @ ..] if !item.is_empty() => Task::Withdraw(text(item)),
        ["deposit", item @ ..] if !item.is_empty() => Task::Deposit(text(item)),
        ["equip", item @ ..] if !item.is_empty() => Task::Equip(text(item)),
        ["buy", shop, rest @ ..] if !rest.is_empty() => {
            let shop = shop.parse().map_err(|_| format!("{shop} is not a shop id"))?;
            let (item, qty) = name_and_count(rest)?;
            Task::Buy { shop, item, qty }
        }
        ["sell", rest @ ..] if !rest.is_empty() => {
            let (item, qty) = name_and_count(rest)?;
            Task::Sell { item, qty }
        }
        ["rest"] => Task::Rest,
        ["accept", quest] => Task::Accept(quest.parse().map_err(|_| format!("{quest} is not a quest id"))?),
        ["complete", quest, reward @ ..] if reward.len() <= 1 => Task::Complete {
            quest: quest.parse().map_err(|_| format!("{quest} is not a quest id"))?,
            reward: reward.first().map(|r| r.parse().map_err(|_| format!("{r} is not an item id"))).transpose()?,
        },
        _ => return Err("expected join MAP [CELL PAD], jump CELL [PAD], kill NAME [COUNT], farm ITEM COUNT [from NAME], accept QUEST, complete QUEST [REWARD], withdraw ITEM, deposit ITEM, buy SHOP ITEM [QTY], sell ITEM [QTY], equip ITEM, pickup ITEM or rest".into()),
    })
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
    let RunArgs { script, account, config, server, record, client_version, json, plain, tasks } = args;
    if let Some(script) = script {
        return Err(format!("can't run {}: scripts aren't supported yet. Run without one to log in", script.display()));
    }
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
        return run_bot(setup, control, recorder, Watch::Status, tasks);
    }
    let (tx, rx) = mpsc::sync_channel(1024);
    output::init(Mode::Tui(tx));
    let slot: tui::Slot = Arc::new(Mutex::new(None));
    let watch = Watch::Snapshots(slot.clone());
    let bot = std::thread::Builder::new()
        .name("bot".into())
        .spawn(move || run_bot(setup, control, recorder, watch, tasks))
        .map_err(|e| e.to_string())?;
    tui::run(rx, slot, stop, bot)
}

fn run_bot(setup: Setup, mut control: Control, mut recorder: Option<Recorder>, watch: Watch, tasks: Vec<Task>) -> Result<(), String> {
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
        match live(&mut session, &tasks) {
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

fn live(session: &mut Session, tasks: &[Task]) -> Result<(), Halt> {
    session.pump_until(Some(Instant::now() + READY_TIMEOUT), |s| (s.phase == Phase::Ready).then_some(()))?;
    for task in tasks {
        say("aqx", format_args!("{task}"));
        match run_task(session, task) {
            Ok(()) => {}
            Err(ActError::Halted(halt)) => return Err(halt),
            Err(e) => {
                say("aqx", format_args!("{task} failed: {e}"));
                break;
            }
        }
    }
    session.pump_until(None, |_| None::<()>)
}

fn run_task(session: &mut Session, task: &Task) -> Result<(), ActError> {
    match task {
        Task::Join { map, cell } => {
            session.join(map)?;
            match cell {
                Some((cell, pad)) => session.jump(cell, pad),
                None => Ok(()),
            }
        }
        Task::Jump { cell, pad } => session.jump(cell, pad),
        Task::Kill { name, count } => {
            let matches = |m: &aqx_core::Monster| name == "*" || m.name.eq_ignore_ascii_case(name);
            let mut fight = Fight::default();
            for _ in 0..*count {
                session.kill(&matches, &mut fight, None, None)?;
            }
            Ok(())
        }
        Task::Farm { item, count, from } => {
            let matches = |m: &aqx_core::Monster| from.as_ref().is_none_or(|name| m.name.eq_ignore_ascii_case(name));
            session.farm(&[(ItemRef::Name(item.clone()), *count)], &matches, &mut Fight::default(), None, None)
        }
        Task::Accept(quest) => session.accept(QuestId(*quest)),
        Task::Complete { quest, reward } => session.complete(QuestId(*quest), reward.map(ItemId), 1),
        Task::Withdraw(item) => session.withdraw(&ItemRef::Name(item.clone())),
        Task::Deposit(item) => session.deposit(&ItemRef::Name(item.clone())),
        Task::Buy { shop, item, qty } => session.buy(ShopId(*shop), &ItemRef::Name(item.clone()), *qty),
        Task::Sell { item, qty } => session.sell(&ItemRef::Name(item.clone()), *qty),
        Task::Equip(item) => session.equip(&ItemRef::Name(item.clone())),
        Task::Pickup(item) => session.pickup(item),
        Task::Rest => session.rest(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_steps_take_an_optional_count_of_at_least_one() {
        assert!(matches!(parse_task("kill Lava Golem 3"), Ok(Task::Kill { name, count: 3 }) if name == "Lava Golem"));
        assert!(matches!(parse_task("kill Lava Golem"), Ok(Task::Kill { name, count: 1 }) if name == "Lava Golem"));
        assert!(matches!(parse_task("kill 7"), Ok(Task::Kill { name, count: 1 }) if name == "7"));
        assert!(parse_task("kill Treeant 0").is_err());
    }

    #[test]
    fn farm_steps_name_an_item_a_count_and_maybe_a_monster() {
        assert!(matches!(parse_task("farm Treeant Branch 3 from Treeant"), Ok(Task::Farm { item, count: 3, from: Some(m) }) if item == "Treeant Branch" && m == "Treeant"));
        assert!(matches!(parse_task("farm Bone Dust 10"), Ok(Task::Farm { item, count: 10, from: None }) if item == "Bone Dust"));
        assert!(parse_task("farm Bone Dust").is_err());
    }

    #[test]
    fn shop_steps_name_a_shop_an_item_and_maybe_a_quantity() {
        assert!(matches!(parse_task("buy 1348 Health Potion 5"), Ok(Task::Buy { shop: 1348, item, qty: 5 }) if item == "Health Potion"));
        assert!(matches!(parse_task("sell Sword"), Ok(Task::Sell { item, qty: 1 }) if item == "Sword"));
        assert!(parse_task("buy shop Sword").is_err());
        assert!(parse_task("sell Sword 0").is_err());
    }
}
