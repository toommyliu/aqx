use std::fmt;
use std::time::{Duration, Instant};

use aqx_protocol::{Aim, AreaId, ItemId, MonMapId, Out, QuestId, ShopId, ShopItemId};

use crate::api::{self, Server};
use crate::event::Event;
use crate::pacer::{self, Kind};
use crate::quests::{QuestStatus, RewardKind};
use crate::session::{Goal, Halt, LoginError, Phase, Session};
use crate::world::{Item, ItemRef, Monster, SkillTarget, State, World, items_from};

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const REST_TIMEOUT: Duration = Duration::from_secs(30);
const PICKUP_TIMEOUT: Duration = Duration::from_secs(5);
const QUEST_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TRIES: usize = 5;
const BANK_TIMEOUT: Duration = Duration::from_secs(30);
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
const JUMP_SETTLE: Duration = Duration::from_millis(500);
const SERVER_MARGIN: Duration = Duration::from_millis(200);
const HOLD_RECHECK: Duration = Duration::from_millis(200);
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
const AFTER_COMBAT: Duration = Duration::from_millis(1500);
const CALM_TRIES: usize = 5;
const MANA_TIMEOUT: Duration = Duration::from_secs(30);
const COLLECT_WAIT: Duration = Duration::from_secs(2);
const COLLECT_TRIES: u32 = 3;
const WALK_SPEED: u32 = 8;
const AUTO_ATTACK: usize = 0;

#[derive(Debug)]
pub enum ActError {
    Halted(Halt),
    Timeout(String),
    NotFound(String),
    Rejected(String),
}

impl From<Halt> for ActError {
    fn from(halt: Halt) -> Self {
        ActError::Halted(halt)
    }
}

impl fmt::Display for ActError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActError::Halted(Halt::Stopped) => write!(f, "stopped"),
            ActError::Halted(Halt::Interrupted) => write!(f, "interrupted"),
            ActError::Halted(Halt::GoalMet) => write!(f, "the goal was met"),
            ActError::Halted(Halt::Timeout) => write!(f, "timed out"),
            ActError::Halted(Halt::Closed(why)) => write!(f, "{why}"),
            ActError::Timeout(what) => write!(f, "timed out {what}"),
            ActError::NotFound(what) => write!(f, "{what} not found"),
            ActError::Rejected(why) => write!(f, "{why}"),
        }
    }
}

pub type Predicate<'a> = Box<dyn FnMut(&World) -> bool + 'a>;
type Pick<'a> = &'a dyn Fn(&World, &Monster) -> bool;

pub struct Step<'a> {
    pub slot: usize,
    pub when: Option<Predicate<'a>>,
}

pub struct Fight<'a> {
    pub steps: Vec<Step<'a>>,
    pub strict: bool,
    pub hold: Option<Predicate<'a>>,
    pub anti_counter: bool,
    next: usize,
}

impl Default for Fight<'_> {
    fn default() -> Self {
        Fight::new((1..=4).map(|slot| Step { slot, when: None }).collect())
    }
}

impl<'a> Fight<'a> {
    pub fn new(steps: Vec<Step<'a>>) -> Self {
        Fight { steps, strict: false, hold: None, anti_counter: true, next: 0 }
    }
}

enum Choice {
    Cast(usize),
    Wait(Instant),
}

impl Session {
    fn ready(&mut self, deadline: Option<Instant>) -> Result<(), ActError> {
        self.wait_for(deadline, "waiting to be alive and ready", |s| {
            s.phase == Phase::Ready && !s.paused() && s.world.me.as_ref().is_some_and(|m| m.vitals.state != State::Dead)
        })
    }

    fn wait_for(&mut self, deadline: Option<Instant>, what: &str, mut done: impl FnMut(&Session) -> bool) -> Result<(), ActError> {
        match self.pump_until(deadline, |s| done(s).then_some(())) {
            Err(Halt::Timeout) => Err(ActError::Timeout(what.into())),
            other => Ok(other?),
        }
    }

    pub fn join(&mut self, map: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let base = map.split('-').next().unwrap_or(map);
        let here = |s: &Session| s.world.area.as_ref().is_some_and(|a| a.map.eq_ignore_ascii_case(base) && s.joined == Some(a.id));
        let same_room = self.world.area.as_ref().is_some_and(|a| a.name.eq_ignore_ascii_case(map));
        if here(self) && (same_room || !map.contains('-')) {
            return Ok(());
        }
        let user = self.world.me.as_ref().map(|m| m.user.clone()).unwrap_or_default();
        let before = self.joined;
        self.send_out_of_combat(
            &format!("joining {map}"),
            JOIN_TIMEOUT,
            |s| s.send_paced(Out::Join { user: &user, map }, Kind::Locked("tfer")),
            |s| here(s) && s.joined != before,
        )
    }

    pub fn jump(&mut self, cell: &str, pad: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Err(ActError::NotFound("a map".into())) };
        if me.cell == cell && me.pad == pad {
            return Ok(());
        }
        let room = area.id;
        self.send_paced(Out::MoveToCell { room, cell, pad }, Kind::Free)?;
        self.moved_to(cell, pad);
        self.idle_until(Instant::now() + JUMP_SETTLE)?;
        Ok(())
    }

    pub fn rest(&mut self) -> Result<(), ActError> {
        self.ready(None)?;
        let full = |s: &Session| s.world.me.as_ref().is_some_and(|m| m.vitals.hp >= m.vitals.max_hp && m.vitals.mp >= m.vitals.max_mp);
        if full(self) {
            return Ok(());
        }
        self.send_paced(Out::Rest, Kind::Locked("rest"))?;
        self.send(Out::Emote("rest"));
        self.wait_for(Some(Instant::now() + REST_TIMEOUT), "resting", full)
    }

    pub fn pickup(&mut self, name: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let Some(item) = self.world.drops.iter().find(|d| d.name.eq_ignore_ascii_case(name)).map(|d| d.id) else {
            return Err(ActError::NotFound(format!("a drop named {name}")));
        };
        let Some(room) = self.world.area.as_ref().map(|a| a.id) else { return Err(ActError::NotFound("a map".into())) };
        self.send_paced(Out::PickUp { room, item }, Kind::Free)?;
        self.wait_for(Some(Instant::now() + PICKUP_TIMEOUT), &format!("picking up {name}"), |s| s.world.drops.iter().all(|d| d.id != item))
    }

    fn room(&self) -> Result<AreaId, ActError> {
        self.world.area.as_ref().map(|a| a.id).ok_or_else(|| ActError::NotFound("a map".into()))
    }

    pub fn load_quests(&mut self, ids: &[QuestId]) -> Result<(), ActError> {
        self.ready(None)?;
        let missing: Vec<QuestId> = ids.iter().copied().filter(|id| !self.world.quests.contains_key(id)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        let room = self.room()?;
        let loaded = |s: &Session| missing.iter().all(|id| s.world.quests.contains_key(id));
        for _ in 0..LOAD_TRIES {
            let slowed = self.slowed;
            self.send_paced(Out::LoadQuests { room, quests: &missing }, Kind::Locked("getQuests"))?;
            self.wait_for(Some(Instant::now() + QUEST_TIMEOUT), "loading quests", |s| loaded(s) || s.slowed != slowed)?;
            if loaded(self) {
                return Ok(());
            }
        }
        Err(ActError::Rejected("the server kept saying to slow down while loading quests".into()))
    }

    pub fn accept(&mut self, id: QuestId) -> Result<(), ActError> {
        self.load_quests(&[id])?;
        let name = self.world.quests.get(&id).map_or_else(|| format!("quest {id}"), |q| q.name.clone());
        match self.world.quest_status(id) {
            Some(QuestStatus::Active | QuestStatus::Completable) => return Ok(()),
            Some(QuestStatus::Done) => return Err(ActError::Rejected(format!("{name} is already done"))),
            Some(QuestStatus::Locked) => return Err(ActError::Rejected(format!("{name} is locked"))),
            _ => {}
        }
        let room = self.room()?;
        self.quest_outcomes.remove(&id);
        self.send_paced(Out::AcceptQuest { room, quest: id }, Kind::Locked("acceptQuest"))?;
        self.quest_verdict(id, "accepting")
    }

    pub fn complete(&mut self, id: QuestId, reward: Option<ItemId>, times: u32) -> Result<(), ActError> {
        self.load_quests(&[id])?;
        let Some(quest) = self.world.quests.get(&id).cloned() else { return Err(ActError::NotFound(format!("quest {id}"))) };
        if quest.once && self.world.quest_status(id) == Some(QuestStatus::Done) {
            return Ok(());
        }
        if !self.world.active.contains(&id) {
            self.accept(id)?;
        }
        let choices: Vec<_> = quest.rewards.iter().filter(|r| r.kind == RewardKind::Choice).collect();
        if !choices.is_empty() && !reward.is_some_and(|item| choices.iter().any(|c| c.item == item)) {
            let names: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
            return Err(ActError::Rejected(format!("{} needs a reward chosen from {}", quest.name, names.join(", "))));
        }
        if self.world.quest_status(id) != Some(QuestStatus::Completable) {
            let missing: Vec<String> =
                quest.turn_in.iter().filter(|n| self.world.count(n.item) < n.qty).map(|n| format!("{} {}/{}", n.name, self.world.count(n.item), n.qty)).collect();
            return Err(ActError::Rejected(format!("{} still needs {}", quest.name, missing.join(", "))));
        }
        let room = self.room()?;
        let reward = if choices.is_empty() { None } else { reward };
        self.quest_outcomes.remove(&id);
        self.send_paced(Out::CompleteQuest { room, quest: id, reward, times: times.max(1) }, Kind::Locked("tryQuestComplete"))?;
        self.quest_verdict(id, "completing")
    }

    fn quest_verdict(&mut self, id: QuestId, what: &str) -> Result<(), ActError> {
        let heard = |s: &Session| s.quest_outcomes.contains_key(&id);
        self.wait_for(Some(Instant::now() + QUEST_TIMEOUT), &format!("{what} quest {id}"), heard)?;
        match self.quest_outcomes.remove(&id) {
            Some(Err(reason)) => Err(ActError::Rejected(reason)),
            _ => Ok(()),
        }
    }

    pub fn farm(
        &mut self,
        goals: &[(ItemRef, u32)],
        target: &dyn Fn(&World, &Monster) -> bool,
        fight: &mut Fight,
        cell: Option<(&str, &str)>,
        timeout: Option<Duration>,
    ) -> Result<(), ActError> {
        let wanted = goals.to_vec();
        let met: Goal = Box::new(move |w| wanted.iter().all(|(item, total)| w.count_of(item) >= *total));
        self.with_goal(met, |s| s.farm_until_met(goals, target, fight, cell, timeout))
    }

    pub fn kill_until(
        &mut self,
        until: Goal,
        target: &dyn Fn(&World, &Monster) -> bool,
        fight: &mut Fight,
        cell: Option<(&str, &str)>,
        timeout: Option<Duration>,
    ) -> Result<(), ActError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        self.with_goal(until, |s| {
            while !s.goal_met() {
                let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
                if left.is_some_and(|l| l.is_zero()) {
                    return Err(ActError::Timeout("killing".into()));
                }
                unless_met(s.kill(target, fight, cell, left))?;
            }
            Ok(())
        })
    }

    fn with_goal<T>(&mut self, goal: Goal, f: impl FnOnce(&mut Self) -> Result<T, ActError>) -> Result<T, ActError> {
        let outer = self.goal.replace(goal);
        let result = f(self);
        self.goal = outer;
        result
    }

    fn farm_until_met(
        &mut self,
        goals: &[(ItemRef, u32)],
        target: &dyn Fn(&World, &Monster) -> bool,
        fight: &mut Fight,
        cell: Option<(&str, &str)>,
        timeout: Option<Duration>,
    ) -> Result<(), ActError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut shown: Vec<Option<u32>> = vec![None; goals.len()];
        let labels: Vec<String> = goals.iter().map(|(item, _)| self.world.name_of(item)).collect();
        loop {
            unless_met(self.pick_up_goals(goals))?;
            let mut done = true;
            for (((item, total), last), label) in goals.iter().zip(&mut shown).zip(&labels) {
                let current = self.world.count_of(item);
                if *last != Some(current) {
                    *last = Some(current);
                    (self.sink)(&Event::Progress { label: label.clone(), current: current.min(*total), total: *total });
                }
                done &= current >= *total;
            }
            if done {
                return Ok(());
            }
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            if left.is_some_and(|l| l.is_zero()) {
                return Err(ActError::Timeout("farming".into()));
            }
            unless_met(self.kill(target, fight, cell, left))?;
        }
    }

    fn pick_up_goals(&mut self, goals: &[(ItemRef, u32)]) -> Result<(), ActError> {
        let wanted: Vec<String> = self
            .world
            .drops
            .iter()
            .filter(|d| goals.iter().any(|(item, total)| item.matches(d) && self.world.count_of(item) < *total))
            .map(|d| d.name.clone())
            .collect();
        for name in wanted {
            self.pickup(&name)?;
        }
        Ok(())
    }

    fn off_thread_for<T: Send + 'static>(&mut self, job: impl FnOnce() -> T + Send + 'static, timeout: Duration, what: &str) -> Result<T, ActError> {
        match self.off_thread(job, timeout) {
            Err(Halt::Timeout) => Err(ActError::Timeout(what.into())),
            other => Ok(other?),
        }
    }

    fn send_expecting(&mut self, action: &str, out: Out, kind: Kind) -> Result<(), ActError> {
        self.refusals.remove(action);
        Ok(self.send_paced(out, kind)?)
    }

    fn refused_or(&mut self, action: &str, what: &str, mut done: impl FnMut(&Session) -> bool) -> Result<(), ActError> {
        let refused = |s: &Session| s.refusals.contains_key(action);
        self.wait_for(Some(Instant::now() + REPLY_TIMEOUT), what, |s| refused(s) || done(s))?;
        if refused(self) {
            return Err(ActError::Rejected(self.refusals.remove(action).unwrap_or_default()));
        }
        Ok(())
    }

    pub fn load_bank(&mut self) -> Result<(), ActError> {
        self.ready(None)?;
        let char_id = self.world.me.as_ref().and_then(|m| m.char_id).ok_or_else(|| ActError::NotFound("the character id".into()))?;
        let (api, token) = (self.api().to_string(), self.token().to_string());
        let body = self
            .off_thread_for(move || api::load_bank(&api, char_id, &token), BANK_TIMEOUT, "loading the bank")?
            .map_err(|e| ActError::Rejected(e.to_string()))?;
        let values: Vec<serde_json::Value> = serde_json::from_str(&body).map_err(|e| ActError::Rejected(format!("unreadable bank: {e}")))?;
        let mut events = Vec::new();
        self.world.set_bank(items_from(values, "bank item", &mut events));
        self.world.rev += 1;
        events.push(Event::BankLoaded { items: self.world.bank.len() });
        for event in &events {
            (self.sink)(event);
        }
        Ok(())
    }

    pub fn withdraw(&mut self, item: &ItemRef) -> Result<(), ActError> {
        self.ready(None)?;
        if self.world.inventory.iter().any(|i| item.matches(i)) {
            return Ok(());
        }
        if !self.world.bank_loaded {
            self.load_bank()?;
        }
        let Some(found) = self.world.bank.iter().find(|i| item.matches(i)).cloned() else {
            return Err(ActError::NotFound(format!("{item} in the bank")));
        };
        if self.bag_full() {
            return Err(ActError::Rejected("the inventory is full".into()));
        }
        let char_item = found.char_item.ok_or_else(|| ActError::NotFound(format!("a character item id for {item}")))?;
        let room = self.room()?;
        self.send_expecting("withdrawing", Out::Withdraw { room, item: found.id, char_item }, Kind::Free)?;
        self.refused_or("withdrawing", &format!("withdrawing {item}"), |s| s.world.inventory.iter().any(|i| i.id == found.id))
    }

    pub fn deposit(&mut self, item: &ItemRef) -> Result<(), ActError> {
        self.ready(None)?;
        let Some(found) = self.world.inventory.iter().find(|i| item.matches(i)).cloned() else {
            if !self.world.bank_loaded {
                self.load_bank()?;
            }
            return if self.world.bank.iter().any(|i| item.matches(i)) { Ok(()) } else { Err(ActError::NotFound(format!("{item} in the inventory"))) };
        };
        if found.equipped {
            return Err(ActError::Rejected(format!("{} is equipped", found.name)));
        }
        if !found.ac && self.world.bank_full() {
            return Err(ActError::Rejected("the bank is full".into()));
        }
        let char_item = found.char_item.ok_or_else(|| ActError::NotFound(format!("a character item id for {item}")))?;
        let room = self.room()?;
        self.send_expecting("depositing", Out::Deposit { room, item: found.id, char_item }, Kind::Free)?;
        self.refused_or("depositing", &format!("depositing {item}"), |s| s.world.bank.iter().any(|i| i.id == found.id))
    }

    fn bag_full(&self) -> bool {
        self.world.me.as_ref().is_some_and(|m| m.bag_slots > 0 && self.world.inventory.len() as u32 >= m.bag_slots)
    }

    pub fn load_shop(&mut self, shop: ShopId) -> Result<(), ActError> {
        self.ready(None)?;
        if self.world.shops.contains_key(&shop) {
            self.world.last_shop = Some(shop);
            return Ok(());
        }
        let room = self.room()?;
        self.send_out_of_combat(
            &format!("loading shop {shop}"),
            REPLY_TIMEOUT,
            |s| s.send_paced(Out::LoadShop { room, shop }, Kind::Locked("loadShop")),
            |s| s.world.shops.contains_key(&shop),
        )
    }

    pub fn buy(&mut self, shop: ShopId, item: &ItemRef, listing: Option<ShopItemId>, qty: u32) -> Result<(), ActError> {
        self.load_shop(shop)?;
        let listed = self.world.shops.get(&shop).and_then(|s| s.items.iter().find(|i| item.matches(&i.item) && listing.is_none_or(|id| i.id == id)));
        let Some(listed) = listed.cloned() else {
            return Err(ActError::NotFound(format!("{item} in shop {shop}")));
        };
        let qty = qty.max(1);
        if listed.item.max_stack <= 1 {
            if !self.world.bank_loaded {
                self.load_bank()?;
            }
            if self.world.count(listed.item.id) + self.banked(listed.item.id) > 0 {
                return Ok(());
            }
        }
        let me = self.world.me.as_ref().ok_or_else(|| ActError::NotFound("a character".into()))?;
        let price = i64::from(listed.cost) * i64::from(qty);
        let funds = if listed.item.ac { me.coins } else { me.gold };
        if listed.item.member && !me.member {
            return Err(ActError::Rejected(format!("{} is for members", listed.item.name)));
        }
        if let Some((faction, rep)) = listed.reputation
            && self.world.factions.iter().find(|f| f.id == faction).is_none_or(|f| f.rep < rep)
        {
            return Err(ActError::Rejected(format!("{} needs more reputation", listed.item.name)));
        }
        if funds < price {
            let currency = if listed.item.ac { "AC" } else { "gold" };
            return Err(ActError::Rejected(format!("{} costs {price} {currency}, you have {funds}", listed.item.name)));
        }
        let before = self.world.count(listed.item.id) + self.banked(listed.item.id);
        let room = self.room()?;
        self.world.buying = Some(shop);
        let out = Out::Buy { room, item: listed.item.id, shop, shop_item: listed.id, qty };
        self.send_expecting("buying", out, Kind::Locked("buyItem"))?;
        let id = listed.item.id;
        self.refused_or("buying", &format!("buying {}", listed.item.name), |s| s.world.count(id) + s.banked(id) > before)
    }

    fn banked(&self, id: ItemId) -> u32 {
        self.world.bank.iter().filter(|i| i.id == id).map(|i| i.qty).sum()
    }

    pub fn sell(&mut self, item: &ItemRef, qty: u32) -> Result<(), ActError> {
        self.ready(None)?;
        let Some(found) = self.world.inventory.iter().find(|i| item.matches(i)).cloned() else {
            return Err(ActError::NotFound(format!("{item} in the inventory")));
        };
        if found.equipped {
            return Err(ActError::Rejected(format!("{} is equipped", found.name)));
        }
        let char_item = found.char_item.ok_or_else(|| ActError::NotFound(format!("a character item id for {item}")))?;
        let qty = qty.max(1).min(found.qty.max(1));
        let room = self.room()?;
        self.send_expecting("selling", Out::Sell { room, item: found.id, qty, char_item }, Kind::Locked("sellItem"))?;
        let left = found.qty.saturating_sub(qty);
        self.refused_or("selling", &format!("selling {}", found.name), |s| s.world.count(found.id) <= left)
    }

    pub fn equip(&mut self, item: &ItemRef) -> Result<(), ActError> {
        self.ready(None)?;
        let Some(found) = self.world.inventory.iter().find(|i| item.matches(i)).cloned() else {
            return Err(ActError::NotFound(format!("{item} in the inventory")));
        };
        let room = self.room()?;
        if consumable(&found) {
            let skill: u32 = found.meta.parse().map_err(|_| ActError::Rejected(format!("{} has no skill to equip", found.name)))?;
            if self.world.me.as_ref().is_some_and(|m| m.item_skill == Some(skill)) {
                return Ok(());
            }
            self.send_paced(Out::EquipConsumable { room, link: &found.link, meta: &found.meta, item: found.id }, Kind::Free)?;
            return self.wait_for(Some(Instant::now() + REPLY_TIMEOUT), &format!("equipping {}", found.name), |s| {
                s.world.me.as_ref().is_some_and(|m| m.item_skill == Some(skill))
            });
        }
        if found.equipped {
            return Ok(());
        }
        self.send_paced(Out::Equip { room, item: found.id }, Kind::Locked("equipItem"))?;
        self.wait_for(Some(Instant::now() + REPLY_TIMEOUT), &format!("equipping {}", found.name), |s| {
            s.world.inventory.iter().any(|i| i.id == found.id && i.equipped)
        })
    }

    pub fn kill(
        &mut self,
        target: &dyn Fn(&World, &Monster) -> bool,
        fight: &mut Fight,
        cell: Option<(&str, &str)>,
        timeout: Option<Duration>,
    ) -> Result<(), ActError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            self.ready(deadline)?;
            if self.world.me.as_ref().is_none_or(|m| m.skills.is_empty()) {
                return Err(ActError::NotFound("a skill list (the server never sent sAct)".into()));
            }
            if let Some((cell, pad)) = cell {
                self.jump(cell, pad)?;
            }
            let Some(id) = self.target_in_cell(target) else {
                if cell.is_none()
                    && let Some(best) = self.best_cell(target)
                    && self.world.me.as_ref().is_some_and(|m| m.cell != best)
                {
                    self.jump(&best, "Left")?;
                    continue;
                }
                let hunting = cell.is_none();
                self.wait_for(deadline, "waiting for a target", |s| {
                    s.target_in_cell(target).is_some() || s.dead() || (hunting && s.live_target_anywhere(target))
                })?;
                continue;
            };
            if self.fight(id, fight, deadline)? {
                return Ok(());
            }
        }
    }

    fn dead(&self) -> bool {
        self.world.me.as_ref().is_none_or(|m| m.vitals.state == State::Dead)
    }

    fn fight(&mut self, id: MonMapId, fight: &mut Fight, deadline: Option<Instant>) -> Result<bool, ActError> {
        self.world.target = Some(id);
        let result = self.fight_target(id, fight, deadline);
        self.world.target = None;
        result
    }

    fn fight_target(&mut self, id: MonMapId, fight: &mut Fight, deadline: Option<Instant>) -> Result<bool, ActError> {
        loop {
            if self.paused() {
                self.wait_for(deadline, "waiting to resume", |s| !s.paused())?;
                continue;
            }
            let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Ok(false) };
            let Some(monster) = area.monsters.iter().find(|m| m.map_id == id) else { return Ok(false) };
            if !monster.alive() {
                return Ok(true);
            }
            if me.vitals.state == State::Dead || me.cell != monster.cell {
                return Ok(false);
            }
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return Err(ActError::Timeout("killing".into()));
            }
            let countering = fight.anti_counter && area.monsters.iter().any(|m| m.cell == me.cell && m.countering(now));
            let holding = fight.hold.as_mut().is_some_and(|hold| hold(&self.world));
            if countering || holding {
                self.idle_until(cap(now + HOLD_RECHECK, deadline))?;
                continue;
            }
            match self.choose(fight, now) {
                Choice::Cast(slot) => self.cast(slot, Some(id))?,
                Choice::Wait(at) => self.idle_until(cap(at, deadline))?,
            }
        }
    }

    fn choose(&self, fight: &mut Fight, now: Instant) -> Choice {
        let mut earliest = now + HOLD_RECHECK;
        let order: Vec<usize> = if fight.strict && !fight.steps.is_empty() { vec![fight.next % fight.steps.len()] } else { (0..fight.steps.len()).collect() };
        for i in order {
            let step = &mut fight.steps[i];
            let Some(at) = self.castable_at(step.slot, now) else { continue };
            if at > now {
                earliest = earliest.min(at);
                continue;
            }
            if step.when.as_mut().is_none_or(|when| when(&self.world)) {
                fight.next = i + 1;
                return Choice::Cast(step.slot);
            }
        }
        match self.castable_at(AUTO_ATTACK, now) {
            Some(at) if at <= now => Choice::Cast(AUTO_ATTACK),
            Some(at) => Choice::Wait(earliest.min(at)),
            None => Choice::Wait(earliest),
        }
    }

    fn castable_at(&self, slot: usize, now: Instant) -> Option<Instant> {
        let me = self.world.me.as_ref()?;
        let skill = me.skills.get(slot)?;
        if skill.locked || (skill.slot.starts_with('i') && self.equipped_item().is_none()) {
            return None;
        }
        if f64::from(skill.mana) * me.stats.get("$cmc").copied().unwrap_or(1.0) > f64::from(me.vitals.mp) {
            return None;
        }
        let own = self.world.cooldown_starts.get(&skill.slot).map_or(now, |start| *start + me.cooldown(skill));
        let kind = if slot == AUTO_ATTACK { Kind::AutoAttack } else { Kind::ClassSkill };
        Some(own.max(self.pacer_ready_at(kind, now)))
    }

    fn equipped_item(&self) -> Option<ItemId> {
        let skill = self.world.me.as_ref()?.item_skill?;
        self.world.inventory.iter().find(|i| i.qty > 0 && i.meta.parse::<u32>().is_ok_and(|meta| meta == skill)).map(|i| i.id)
    }

    fn cast(&mut self, slot: usize, target: Option<MonMapId>) -> Result<(), ActError> {
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Ok(()) };
        let Some(skill) = me.skills.get(slot) else { return Ok(()) };
        let aims: Vec<Aim> = match (skill.target, target) {
            (SkillTarget::Hostile, Some(id)) => {
                let mut others: Vec<&Monster> = area.monsters.iter().filter(|m| m.map_id != id && m.cell == me.cell && m.alive()).collect();
                others.sort_by_key(|m| m.vitals.hp);
                std::iter::once(id).chain(others.iter().map(|m| m.map_id)).take(skill.max_targets as usize).map(Aim::Monster).collect()
            }
            (SkillTarget::Hostile, None) => return Ok(()),
            (SkillTarget::Friendly | SkillTarget::Myself, _) => vec![Aim::Player(me.eid)],
        };
        let name = skill.slot.clone();
        let kind = if slot == AUTO_ATTACK { Kind::AutoAttack } else { Kind::ClassSkill };
        if name.starts_with('i') {
            let Some(item) = self.equipped_item() else { return Ok(()) };
            let counter = self.next_counter();
            self.send_paced(Out::UseItem { counter, target: aims[0], item }, kind)?;
        } else {
            let counter = self.next_counter();
            self.send_paced(Out::Skill { counter, slot: &name, targets: &aims }, kind)?;
        }
        self.world.cooldown_starts.insert(name, Instant::now() + SERVER_MARGIN);
        Ok(())
    }

    pub fn use_skill(&mut self, slot: usize, target: Option<Pick>) -> Result<(), ActError> {
        let mana_by = Instant::now() + MANA_TIMEOUT;
        loop {
            self.ready(None)?;
            let Some(skill) = self.world.me.as_ref().and_then(|m| m.skills.get(slot)).cloned() else {
                return Err(ActError::NotFound(format!("a skill in slot {slot}")));
            };
            if skill.locked {
                return Err(ActError::Rejected(format!("{} is locked at this class rank", skill.name)));
            }
            if skill.slot.starts_with('i') && self.equipped_item().is_none() {
                return Err(ActError::NotFound("a consumable equipped in slot 5".into()));
            }
            let now = Instant::now();
            let Some(at) = self.castable_at(slot, now) else {
                let what = format!("waiting for mana for {}", skill.name);
                self.wait_for(Some(mana_by), &what, |s| s.castable_at(slot, Instant::now()).is_some())?;
                continue;
            };
            if at > now {
                self.idle_until(at)?;
                continue;
            }
            let aim = match skill.target {
                SkillTarget::Hostile => {
                    let any = |_: &World, _: &Monster| true;
                    let Some(id) = self.target_in_cell(target.unwrap_or(&any)) else {
                        return Err(ActError::NotFound("a live monster to target in this cell".into()));
                    };
                    Some(id)
                }
                SkillTarget::Friendly | SkillTarget::Myself => None,
            };
            return self.cast(slot, aim);
        }
    }

    pub fn exit_combat(&mut self) -> Result<(), ActError> {
        self.ready(None)?;
        let in_combat = |s: &Session| s.world.me.as_ref().is_some_and(|m| m.vitals.state == State::Combat);
        if !in_combat(self) {
            return Ok(());
        }
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return Err(ActError::NotFound("a map".into())) };
        let (room, cell, pad) = (area.id, me.cell.clone(), me.pad.clone());
        self.send_paced(Out::MoveToCell { room, cell: &cell, pad: &pad }, Kind::Free)?;
        self.wait_for(Some(Instant::now() + EXIT_TIMEOUT), "leaving combat", |s| !in_combat(s))
    }

    fn out_of_combat(&mut self) -> Result<(), ActError> {
        let in_combat = |s: &Session| s.world.me.as_ref().is_some_and(|m| m.vitals.state == State::Combat).then_some(());
        for _ in 0..CALM_TRIES {
            self.exit_combat()?;
            let Some(calm) = self.world.me.as_ref().and_then(|m| m.left_combat).map(|left| left + AFTER_COMBAT) else { return Ok(()) };
            match self.pump_until(Some(calm), in_combat) {
                Err(Halt::Timeout) => return Ok(()),
                Ok(()) => continue,
                Err(halt) => return Err(halt.into()),
            }
        }
        Err(ActError::Rejected("the player keeps being pulled back into combat".into()))
    }

    fn send_out_of_combat(
        &mut self,
        what: &str,
        timeout: Duration,
        mut send: impl FnMut(&mut Session) -> Result<(), Halt>,
        arrived: impl Fn(&Session) -> bool,
    ) -> Result<(), ActError> {
        for _ in 0..CALM_TRIES {
            self.out_of_combat()?;
            let slowed = self.slowed;
            send(self)?;
            let answered = |s: &Session| {
                if arrived(s) {
                    Some(true)
                } else if s.slowed != slowed || s.world.me.as_ref().is_some_and(|m| m.vitals.state == State::Combat) {
                    Some(false)
                } else {
                    None
                }
            };
            match self.pump_until(Some(Instant::now() + timeout), answered) {
                Ok(true) => return Ok(()),
                Ok(false) => continue,
                Err(Halt::Timeout) => return Err(ActError::Timeout(what.into())),
                Err(halt) => return Err(halt.into()),
            }
        }
        Err(ActError::Rejected(format!("the player keeps being pulled back into combat while {what}")))
    }

    pub fn walk(&mut self, x: i32, y: i32) -> Result<(), ActError> {
        self.ready(None)?;
        let room = self.room()?;
        self.send_paced(Out::Walk { room, x, y, speed: WALK_SPEED }, Kind::Free)?;
        if let Some(me) = &mut self.world.me {
            (me.x, me.y) = (x, y);
        }
        self.world.rev += 1;
        Ok(())
    }

    pub fn goto(&mut self, name: &str) -> Result<(), ActError> {
        self.ready(None)?;
        let spot = |s: &Session| {
            let area = s.world.area.as_ref()?;
            area.players.iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| (p.cell.clone(), p.pad.clone()))
        };
        if spot(self).is_none() {
            let before = self.joined;
            self.send_out_of_combat(
                &format!("going to {name}"),
                JOIN_TIMEOUT,
                |s| s.send_paced(Out::Goto { name }, Kind::Free),
                |s| s.joined.is_some() && s.joined != before,
            )?;
        }
        let Some((cell, pad)) = spot(self) else { return Err(ActError::NotFound(format!("{name} in this map"))) };
        if cell.is_empty() {
            return Ok(());
        }
        self.jump(&cell, if pad.is_empty() { "Left" } else { &pad })
    }

    pub fn collect(&mut self, map_item: u32, item: &ItemRef, qty: u32) -> Result<(), ActError> {
        let mut misses = 0;
        loop {
            self.ready(None)?;
            let have = self.world.count_of(item);
            if have >= qty {
                return Ok(());
            }
            let room = self.room()?;
            self.send_paced(Out::GetMapItem { room, id: map_item }, Kind::Locked("getMapItem"))?;
            match self.wait_for(Some(Instant::now() + COLLECT_WAIT), "collecting", |s| s.world.count_of(item) > have) {
                Ok(()) => misses = 0,
                Err(ActError::Timeout(_)) if misses + 1 < COLLECT_TRIES => misses += 1,
                Err(ActError::Timeout(_)) => {
                    return Err(ActError::Rejected(format!("map item {map_item} gave no {item} in {COLLECT_TRIES} tries")));
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn use_item(&mut self, item: &ItemRef) -> Result<(), ActError> {
        self.ready(None)?;
        let Some(found) = self.world.inventory.iter().find(|i| item.matches(i) && i.qty > 0).cloned() else {
            return Err(ActError::NotFound(format!("{item} in the inventory")));
        };
        if !found.kind.eq_ignore_ascii_case("ServerUse") {
            return Err(ActError::Rejected(format!("{} isn't a usable item; equip consumables and use slot 5", found.name)));
        }
        Ok(self.send_paced(Out::ServerUseItem { item: found.id }, Kind::Free)?)
    }

    pub fn send_raw(&mut self, text: &str) -> Result<(), ActError> {
        self.ready(None)?;
        if text.contains('\0') {
            return Err(ActError::Rejected("a packet can't contain a NUL character".into()));
        }
        let kind = if let Some(rest) = text.strip_prefix("%xt%") {
            let fields: Vec<&str> = rest.split('%').collect();
            match fields.get(1) {
                Some(&"cmd") => pacer::kind_for(fields.get(3).copied().unwrap_or_default()),
                Some(cmd) => pacer::kind_for(cmd),
                None => Kind::Free,
            }
        } else if text.starts_with('<') || text.starts_with('{') {
            Kind::Free
        } else {
            return Err(ActError::Rejected("a packet starts with %xt%, < or {".into()));
        };
        Ok(self.send_paced(Out::Raw(text), kind)?)
    }

    pub fn servers(&mut self) -> Result<Vec<Server>, ActError> {
        let url = self.api().to_string();
        self.off_thread_for(move || api::servers(&url), REPLY_TIMEOUT, "loading the server list")?.map_err(|e| ActError::Rejected(e.to_string()))
    }

    pub fn relogin(&mut self, server: Option<&str>) -> Result<(), ActError> {
        let name = server.map(String::from).or_else(|| self.server().map(|s| s.name.clone()));
        self.login(name.as_deref()).map_err(|e| match e {
            LoginError::Halted(halt) => ActError::Halted(halt),
            e => ActError::Rejected(e.to_string()),
        })
    }

    fn live_target_anywhere(&self, target: &dyn Fn(&World, &Monster) -> bool) -> bool {
        self.world.area.as_ref().is_some_and(|a| a.monsters.iter().any(|m| m.alive() && target(&self.world, m)))
    }

    fn target_in_cell(&self, target: &dyn Fn(&World, &Monster) -> bool) -> Option<MonMapId> {
        let (me, area) = (self.world.me.as_ref()?, self.world.area.as_ref()?);
        area.monsters.iter().filter(|m| m.cell == me.cell && m.alive() && target(&self.world, m)).min_by_key(|m| m.vitals.hp).map(|m| m.map_id)
    }

    fn best_cell(&self, target: &dyn Fn(&World, &Monster) -> bool) -> Option<String> {
        let area = self.world.area.as_ref()?;
        let mut cells: Vec<(&str, usize, usize)> = Vec::new();
        for m in area.monsters.iter().filter(|m| target(&self.world, m)) {
            match cells.iter_mut().find(|(c, _, _)| *c == m.cell) {
                Some((_, alive, total)) => {
                    *alive += usize::from(m.alive());
                    *total += 1;
                }
                None => cells.push((&m.cell, usize::from(m.alive()), 1)),
            }
        }
        cells.into_iter().max_by_key(|(_, alive, total)| (*alive, *total)).map(|(c, _, _)| c.to_string())
    }
}

fn consumable(item: &Item) -> bool {
    item.kind == "Item" && !item.link.is_empty() && !item.link.eq_ignore_ascii_case("none")
}

fn unless_met(result: Result<(), ActError>) -> Result<(), ActError> {
    match result {
        Err(ActError::Halted(Halt::GoalMet)) => Ok(()),
        other => other,
    }
}

fn cap(at: Instant, deadline: Option<Instant>) -> Instant {
    deadline.map_or(at, |d| at.min(d))
}
