use std::cmp::Reverse;
use std::time::{Duration, Instant};

use aqx_client::{CombatState, Disconnect, Item, ItemId, Load, Me, Phase, Pool, Readiness, World};

use serde::{Deserialize, Serialize};

use crate::fleet::{Account, Board, Ended, Link, Run, Stats};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shown {
    pub name: String,
    pub user: String,
    pub state: State,
    pub server: String,
    pub script: Option<String>,
    pub status: String,
    pub up: Option<Duration>,
    pub stats: Stats,
    pub gold_gained: Option<i64>,
    pub game: Option<Game>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    Connecting,
    LoggingIn,
    Joining,
    Loading,
}

impl State {
    pub fn word(&self) -> &'static str {
        match self {
            State::Offline { .. } => "offline",
            State::Busy(step) => step.label(),
            State::Reconnecting(_) => "reconnecting",
            State::Online => "online",
            State::Running => "running",
            State::Paused => "paused",
            State::Stalled => "stalled",
        }
    }
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::Connecting => "connecting",
            Step::LoggingIn => "logging in",
            Step::Joining => "joining",
            Step::Loading => "loading",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Offline { why: Option<String> },
    Busy(Step),
    Reconnecting(String),
    Online,
    Running,
    Paused,
    Stalled,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Game {
    pub area: String,
    pub cell: String,
    pub pad: String,
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub dead: bool,
    pub away: bool,
    pub class: String,
    pub class_rank: u32,
    pub level: u32,
    pub gold: u64,
    pub skills: Vec<SkillState>,
    pub auras: Vec<Aura>,
    pub target: Option<Target>,
    pub players: Vec<Player>,
    pub drops: Vec<Stack>,
    pub quests: Vec<QuestProgress>,
    pub factions: Vec<Standing>,
    pub temp: Vec<Stack>,
    pub inventory: Vec<Stack>,
    pub bag_used: u32,
    pub misc_used: u32,
    pub bag_slots: u32,
    pub bank: Option<Vec<Stack>>,
    pub bank_used: u32,
    pub bank_slots: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillState {
    pub name: String,
    pub ready: f32,
    pub left_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aura {
    pub name: String,
    pub stacks: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub hp: u32,
    pub max_hp: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    pub name: String,
    pub level: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stack {
    #[serde(with = "item_id")]
    pub id: ItemId,
    pub name: String,
    pub qty: u32,
    pub max: u32,
    pub equipped: bool,
    pub rank: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestProgress {
    pub id: i64,
    pub name: String,
    pub have: u32,
    pub need: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub name: String,
    pub rank: u32,
    pub rep: u32,
    pub next: Option<u32>,
    pub gained: bool,
}

impl Shown {
    pub fn of(account: &Account, full: bool, now: Instant) -> Shown {
        let board = account.board();
        let (state, server, up, world) = match &board.link {
            Link::Offline { why } => (
                State::Offline { why: why.clone() },
                String::new(),
                None,
                None,
            ),
            Link::Connecting => (State::Busy(Step::LoggingIn), String::new(), None, None),
            Link::Waiting { why, until } => (
                State::Reconnecting(format!(
                    "in {} s: {why}",
                    until.saturating_duration_since(now).as_secs()
                )),
                String::new(),
                None,
                None,
            ),
            Link::Session {
                session,
                server,
                since,
            } => {
                let world = session.snapshot();
                let state = state(
                    &world.phase,
                    board.running(),
                    board.paused(),
                    board.stalled(),
                );
                let up = matches!(
                    state,
                    State::Online | State::Running | State::Paused | State::Stalled
                )
                .then(|| now.saturating_duration_since(*since));
                (state, server.clone(), up, Some(world))
            }
        };
        let online = matches!(
            state,
            State::Online | State::Running | State::Paused | State::Stalled
        );
        let script = match &board.run {
            Run::Idle => None,
            Run::Starting { script }
            | Run::Running { script, .. }
            | Run::Restarting { script, .. }
            | Run::Ended { script, .. } => Some(script.display().to_string()),
        };
        let status = match &board.run {
            Run::Starting { .. } => "starting".into(),
            Run::Running { .. } => board.status.clone(),
            Run::Restarting { at, .. } => format!(
                "restarting in {} s",
                at.saturating_duration_since(now).as_secs()
            ),
            Run::Ended {
                how: Ended::Finished,
                ..
            } => "finished".into(),
            Run::Ended {
                how: Ended::Stopped(_),
                ..
            } => "stopped".into(),
            Run::Ended {
                how: Ended::Failed(why),
                ..
            } => format!("failed: {why}"),
            Run::Ended {
                how: Ended::Closed(_),
                ..
            } => "the session closed".into(),
            Run::Idle => String::new(),
        };
        let game = world
            .as_deref()
            .filter(|_| online)
            .and_then(|w| game(w, &board, full, now));
        let gold_gained = match (&game, board.stats.gold_from) {
            (Some(g), Some(from)) => Some(g.gold as i64 - from as i64),
            _ => None,
        };
        Shown {
            name: account.name.clone(),
            user: account.user.clone(),
            state,
            server,
            script,
            status,
            up,
            stats: board.stats,
            gold_gained,
            game,
        }
    }
}

fn state(phase: &Phase, running: bool, paused: bool, stalled: bool) -> State {
    match phase {
        Phase::Connecting { .. } => State::Busy(Step::Connecting),
        Phase::Handshaking | Phase::LoggingIn => State::Busy(Step::LoggingIn),
        Phase::Loading { step } => State::Busy(match step {
            Load::Joining => Step::Joining,
            Load::Character | Load::Inventory => Step::Loading,
        }),
        Phase::Ready if paused => State::Paused,
        Phase::Ready if stalled => State::Stalled,
        Phase::Ready if running => State::Running,
        Phase::Ready => State::Online,
        Phase::Reconnecting {
            reason, wait_ms, ..
        } => State::Reconnecting(format!(
            "in {} s: {reason}",
            Duration::from_millis(*wait_ms).as_secs()
        )),
        Phase::Closed {
            reason: Disconnect::LoggedOut,
        } => State::Offline { why: None },
        Phase::Closed { reason } => State::Offline {
            why: Some(reason.to_string()),
        },
    }
}

fn game(world: &World, board: &Board, full: bool, now: Instant) -> Option<Game> {
    let me = world.me.as_ref()?;
    let mut game = summary(world, me, now);
    if full {
        lists(world, me, board, &mut game);
    }
    Some(game)
}

fn summary(world: &World, me: &Me, now: Instant) -> Game {
    let room = world.room.as_ref();
    let used = |pool| world.inventory.iter().filter(|i| i.pool() == pool).count() as u32;
    Game {
        area: room.map(|r| r.name.clone()).unwrap_or_default(),
        cell: room.map(|r| r.cell.clone()).unwrap_or_default(),
        pad: room.map(|r| r.pad.clone()).unwrap_or_default(),
        hp: me.hp,
        max_hp: me.max_hp,
        mp: me.mp,
        max_mp: me.max_mp,
        dead: me.state == CombatState::Dead,
        away: me.away,
        class: me.class_name.clone(),
        class_rank: me.class_rank(),
        level: me.level,
        gold: me.gold,
        skills: me
            .skills
            .iter()
            .filter(|s| s.slot.starts_with('a') && s.slot != "aa")
            .map(|s| {
                let readiness = world.readiness.get(&s.slot);
                let left = match readiness {
                    Some(Readiness::At(at)) if s.cooldown_ms > 0 => {
                        at.saturating_duration_since(now)
                    }
                    _ => Duration::ZERO,
                };
                SkillState {
                    name: s.name.clone(),
                    ready: match readiness {
                        Some(Readiness::Ready | Readiness::At(_)) | None => (1.0
                            - left.as_secs_f32() * 1000.0 / s.cooldown_ms.max(1) as f32)
                            .clamp(0.0, 1.0),
                        Some(_) => 0.0,
                    },
                    left_ms: u32::try_from(left.as_millis()).unwrap_or(u32::MAX),
                }
            })
            .collect(),
        auras: me
            .auras
            .active
            .iter()
            .map(|a| Aura {
                name: a.name.clone(),
                stacks: a.stacks,
            })
            .collect(),
        target: world.target.and_then(|id| {
            let m = room?.monsters.iter().find(|m| m.id == id)?;
            Some(Target {
                name: m.name.clone(),
                hp: m.hp,
                max_hp: m.max_hp,
            })
        }),
        bag_used: used(Pool::Bag),
        misc_used: used(Pool::Misc),
        bag_slots: me.bag_slots,
        bank_used: me.bank_count,
        bank_slots: me.bank_slots,
        ..Game::default()
    }
}

fn lists(world: &World, me: &Me, board: &Board, game: &mut Game) {
    let latest_first = |mut stacks: Vec<Stack>| {
        stacks.sort_by_key(|s| Reverse(board.touched.get(&s.id).copied()));
        stacks
    };
    let held = |id: ItemId| {
        world
            .inventory
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.qty)
            .or_else(|| world.temp.iter().find(|i| i.id == id).map(|i| i.qty))
            .unwrap_or(0)
    };
    let item = |i: &Item| Stack {
        id: i.id,
        name: i.name.clone(),
        qty: i.qty,
        max: i.max_stack,
        equipped: i.equipped,
        rank: i.class_rank(),
    };
    if let Some(room) = &world.room {
        game.players = room
            .players
            .iter()
            .filter(|p| p.eid != me.id && p.cell.as_deref() == Some(room.cell.as_str()))
            .map(|p| Player {
                name: p.name.clone(),
                level: p.level,
            })
            .collect();
    }
    game.drops = latest_first(
        world
            .drops
            .iter()
            .map(|d| Stack {
                id: d.id,
                name: d.name.clone(),
                qty: d.qty,
                max: d.max_stack,
                equipped: false,
                rank: None,
            })
            .collect(),
    );
    game.quests = world
        .accepted_quests
        .iter()
        .filter_map(|id| world.quests.get(id))
        .map(|q| QuestProgress {
            id: q.id.0,
            name: q.name.clone(),
            have: q.turn_in.iter().map(|n| held(n.id).min(n.qty)).sum(),
            need: q.turn_in.iter().map(|n| n.qty).sum(),
        })
        .collect();
    game.factions = world
        .factions
        .iter()
        .map(|f| Standing {
            name: f.name.clone(),
            rank: f.rank,
            rep: f.rank_progress,
            next: f.rank_progress_max,
            gained: board
                .rep_from
                .get(&f.name)
                .is_some_and(|&from| f.rep > from),
        })
        .collect();
    game.factions
        .sort_by(|a, b| b.gained.cmp(&a.gained).then_with(|| a.name.cmp(&b.name)));
    game.temp = latest_first(
        world
            .temp
            .iter()
            .map(|t| Stack {
                id: t.id,
                name: t.name.clone(),
                qty: t.qty,
                max: t.max_stack,
                equipped: false,
                rank: None,
            })
            .collect(),
    );
    game.inventory = latest_first(world.inventory.iter().map(item).collect());
    game.inventory.sort_by_key(|s| !s.equipped);
    game.bank = world
        .bank
        .as_ref()
        .map(|bank| latest_first(bank.iter().map(item).collect()));
}

pub fn time_of_day(at_ms: u64) -> String {
    let secs = (at_ms / 1000) as libc::time_t;
    let ms = at_ms % 1000;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut local) }.is_null() {
        let day = at_ms / 1000 % 86_400;
        return format!(
            "{:02}:{:02}:{:02}.{ms:03}",
            day / 3600,
            day % 3600 / 60,
            day % 60
        );
    }
    format!(
        "{:02}:{:02}:{:02}.{ms:03}",
        local.tm_hour, local.tm_min, local.tm_sec
    )
}

mod item_id {
    use aqx_client::ItemId;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &ItemId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(id.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ItemId, D::Error> {
        i64::deserialize(d).map(ItemId)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aqx_client::testing::{Harness, Peer, json_message};
    use aqx_client::{Event, Options};
    use aqx_protocol::packet;
    use serde_json::json;

    use super::*;

    #[test]
    fn an_online_account_shows_where_it_is_and_what_it_holds() {
        let harness = Harness::start(Options::default(), |listener| {
            let mut peer = Peer::accept(&listener);
            peer.serve_to_ready();
            for (id, name) in [(300, "Treeant Branch"), (301, "Bone Dust")] {
                peer.send(&json_message(json!({"cmd": "dropItem", "items": {
                    id.to_string(): {"ItemID": id, "sName": name, "iQty": 1, "iStk": 50, "sType": "Item", "sES": "None"},
                }})));
            }
            peer.send(&json_message(json!({
                "cmd": "addGoldExp", "intGold": 0, "intExp": 0, "iRep": 2600, "FactionID": 46, "typ": "q",
            })));
            peer.join_farm();
            peer.expect(&packet::logout());
        });
        let account = Account::detached("player1");
        {
            let mut board = account.board();
            board.link = Link::Session {
                session: harness.session.clone(),
                server: "Test".into(),
                since: Instant::now(),
            };
            board.rep_from = HashMap::from([("Good".to_owned(), 1000)]);
        }
        for event in harness.events.iter() {
            account.board().record(&event);
            if matches!(&event, Event::Joined { map, .. } if map == "farm-1") {
                break;
            }
        }

        let shown = Shown::of(&account, true, Instant::now());
        assert_eq!(shown.state, State::Online);
        assert_eq!((shown.stats.drops, shown.gold_gained), (2, Some(0)));
        let game = shown.game.expect("an online account has a game");
        assert_eq!(
            (game.area.as_str(), game.cell.as_str(), game.pad.as_str()),
            ("farm-1", "Enter", "Spawn")
        );
        assert_eq!(
            (game.hp, game.max_hp, game.level, game.bag_slots),
            (1000, 1000, 50, 2)
        );
        let names = |stacks: &[Stack]| stacks.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(&game.drops),
            ["Bone Dust", "Treeant Branch"],
            "the newest drop comes first"
        );
        assert_eq!(
            names(&game.inventory),
            ["Default Sword", "Health Potion"],
            "equipped items lead"
        );
        let good = &game.factions[0];
        assert_eq!((good.name.as_str(), good.gained), ("Good", true));

        let summary = Shown::of(&account, false, Instant::now()).game.unwrap();
        assert!(
            summary.drops.is_empty() && summary.inventory.is_empty(),
            "only the selected account builds its lists"
        );
        assert_eq!(summary.hp, 1000);

        harness.session.logout();
        harness.finish();
    }
}
