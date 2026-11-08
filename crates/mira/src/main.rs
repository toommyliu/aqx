use std::collections::{BTreeMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use mira_client::{
    ActionError, Aggro, Aim, Auras, BoostKind, CombatState, CounterHold,
    DEFAULT_RECONNECT_ATTEMPTS, DeviceId, Disconnect, Event, Handle, Item, ItemId, MapItemId, Me,
    MonMapId, Monster, Options, Pending, Phase, QuestId, QuestStatus, Readiness, Reconnect, Room,
    Server, ShopId, ShopItemId, SkillTarget, World, api,
};
use mira_script::{Cancel, Level, Outcome, Output};

mod commands;
mod config;
mod control;
mod daemon;
mod describe;
mod fleet;
mod init;
mod record;
mod runner;
mod tui;
mod view;

use describe::describe;
use runner::{Job, Login, Note};

const DEFAULT_PAD: &str = "Left";
const COMMANDS: &str = "join <map> [cell] [pad], cell <cell> [pad], attack <monster>, skill <slot> [monster|player], \
     kill <monster>, untarget, stop, cancel, who, cells, mons [all], me, inv, drops, pick <item>, reps, quests, \
     load <quest>..., accept <quest>, complete <quest> [times] [reward], shop <id>, \
     buy <#listing> [qty], sell <item> [qty], bank, deposit <item>, withdraw <item>, \
     swap <#inventory item> <#bank item>, equip <item>, unequip <item>, use <item>, mapitem <id> [times], rest, goto <player>, walk <x> <y>, spawnpoint <cell> [pad], room <number>|random, antiafk on|off, aggro on|off|<monster>[, <monster>...], anticounter off|all|attacks, logout";

#[derive(Parser)]
#[command(
    name = "mira",
    version,
    about = "A headless AQW client. Without a command, opens the TUI",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    open: commands::Open,
    /// Show accounts as "account 1", "account 2" and the home folder as ~, for screenshots.
    #[arg(long)]
    redact: bool,
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
    /// (join <map> [cell] [pad], cell <cell> [pad], attack <monster>, skill <slot> [monster|player], kill <monster>, untarget, stop, cancel,
    /// who, mons [all], me, inv, drops, pick <item>, reps, quests, load <quest>..., accept <quest>,
    /// complete <quest> [times] [reward], shop <id>, buy <#listing> [qty], sell <item> [qty], bank,
    /// deposit <item>, withdraw <item>, swap <#inventory item> <#bank item>, equip <item>,
    /// unequip <item>, use <item>, mapitem <id> [times], rest, goto <player>, walk <x> <y>,
    /// spawnpoint <cell> [pad], room <number>|random, antiafk on|off, aggro on|off|<monster>[, <monster>...], anticounter off|all|attacks, logout)
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
    /// Set up a folder for scripts: mira.d.ts, tsconfig.json, a sample script and mira.toml.
    Init {
        /// Where to write. Defaults to the folder of the config mira tui would read, else $MIRA_HOME or ~/mira.
        dir: Option<PathBuf>,
    },
    /// Open the TUI, the same as mira without a command.
    Tui {
        #[command(flatten)]
        open: commands::Open,
        /// Show accounts as "account 1", "account 2" and the home folder as ~, for screenshots.
        #[arg(long)]
        redact: bool,
    },
    /// Start a session in the background without opening the TUI.
    Up {
        #[command(flatten)]
        open: commands::Open,
    },
    /// Run a session in the foreground, for systemd. mira starts one in the background on its own.
    Serve {
        #[command(flatten)]
        open: commands::Open,
        /// Print the log as JSON lines.
        #[arg(long)]
        json: bool,
        /// Print nothing; read the log with mira logs or the TUI.
        #[arg(long, conflicts_with = "json")]
        quiet: bool,
    },
    /// Log out every account and end the session.
    Down {
        /// The config whose session to end, instead of ./mira.toml or ~/mira/mira.toml.
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
    /// Download, update, and search the Skua script library in $MIRA_HOME/skua or ~/mira/skua.
    Skua {
        #[command(subcommand)]
        action: commands::Skua,
    },
    /// Print the JSON Schema of mira.toml, for editor completion and checks.
    Schema,
    /// Print a run --record file one packet per line.
    View {
        file: PathBuf,
        /// Only show these commands, like --cmd moveToArea --cmd goto.
        #[arg(long, value_name = "NAME")]
        cmd: Vec<String>,
        /// Don't cut lines at 160 characters.
        #[arg(long)]
        full: bool,
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
        /// Server name, as `mira servers` lists it. With --account, defaults to the account's server.
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
    let Some(command) = cli.command else {
        return finish(commands::open(&cli.open, cli.redact));
    };
    finish(match command {
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
        Command::Init { dir } => commands::init(dir),
        Command::Tui { open, redact } => commands::open(&open, redact),
        Command::Up { open } => commands::up(&open),
        Command::Serve { open, json, quiet } => commands::serve(&open, json, quiet),
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
        Command::Schema => {
            println!("{}", config::schema());
            Ok(ExitCode::SUCCESS)
        }
        Command::View { file, cmd, full } => {
            record::view(&file, &cmd, full).map(|()| ExitCode::SUCCESS)
        }
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
        eprintln!("mira: {e:#}");
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
            reconnect: None,
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
        reconnect: None,
        api: account.api,
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
    let login: Arc<dyn mira_script::Account> = Arc::new(login);
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
            Note::Relogging {
                server,
                delay,
                attempt,
                attempts,
            } => println!(
                "{}the session closed; logging back in to {server} in {:.1} s (attempt {attempt} of {attempts})",
                stamp(started),
                delay.as_secs_f64(),
            ),
            Note::ReloginFailed(e) => eprintln!("mira: couldn't log back in: {e}"),
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
            eprintln!("mira: the session closed: {reason}");
            ExitCode::FAILURE
        }
        Outcome::Failed(why) => {
            eprintln!("mira: {why}");
            ExitCode::FAILURE
        }
        Outcome::Restart | Outcome::Relogin(_) => unreachable!("handled by the runner"),
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
        ["skill", slot, target @ ..] => {
            let target = find_skill_target(session, slot, &target.join(" "))?;
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
        ["cells"] => {
            session.load_cells().wait()?;
            let world = session.snapshot();
            let room = world.room.as_ref().ok_or(ActionError::NotReady)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&room.cells).expect("cells serialize")
                );
            } else {
                println!("{}", room.cells.join(", "));
            }
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
        ["room", "random"] => session.set_room(None),
        ["room", room] => {
            let room = room
                .parse()
                .map_err(|_| CommandError::Usage(format!("{room:?} isn't a room number")))?;
            session.set_room(Some(room));
        }
        ["antiafk", "on"] => session.set_anti_afk(true),
        ["antiafk", "off"] => session.set_anti_afk(false),
        ["spawnpoint", cell] => session.set_spawn_point(cell, DEFAULT_PAD),
        ["spawnpoint", cell, pad] => session.set_spawn_point(cell, pad),
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

fn find_skill_target(session: &Handle, slot: &str, wanted: &str) -> Result<Aim, CommandError> {
    let world = session.snapshot();
    let (Some(me), Some(room)) = (&world.me, &world.room) else {
        return Err(CommandError::Action(ActionError::NotReady));
    };
    let friendly = me
        .skills
        .iter()
        .any(|s| s.slot == slot && matches!(s.target, SkillTarget::Friendly));
    if !friendly {
        return find_monster(session, wanted).map(Aim::Monster);
    }
    if me.name.eq_ignore_ascii_case(wanted) {
        return Ok(Aim::Player(me.id));
    }
    room.players
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(wanted))
        .map(|p| Aim::Player(p.eid))
        .ok_or_else(|| CommandError::Usage(format!("no player {wanted} in this map")))
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
