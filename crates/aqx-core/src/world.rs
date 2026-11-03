use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqx_protocol::de::{flag, int, text};
use aqx_protocol::{AreaId, CharItemId, EntityId, FactionId, ItemId, Json, Message, MonId, MonMapId, QuestId, ShopId, ShopItemId, Xml, Xt};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::event::{Event, Place};
use crate::quests::{Quest, QuestData, set_quest_value};

const HASTE_CAP: f64 = 0.5;
const RANK_POINTS: [u32; 9] = [900, 3600, 10000, 22500, 44100, 78400, 129600, 202500, 302500];
const GOLD_CAP: i64 = 100_000_000;
const DEFAULT_MISC_SLOTS: i64 = 100;
const EMAIL_VERIFIED: i64 = 5;
const COUNTER_AURA: &str = "Counter Attack";
const COUNTER_WARNING: &str = "prepares a counter attack";
const COUNTER_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct World {
    pub me: Option<Me>,
    pub area: Option<Area>,
    pub inventory: Vec<Item>,
    pub house: Vec<Item>,
    pub temp: Vec<Item>,
    pub factions: Vec<Faction>,
    pub drops: Vec<Item>,
    pub quests: HashMap<QuestId, Quest>,
    pub active: Vec<QuestId>,
    pub bank: Arc<Vec<Item>>,
    pub bank_loaded: bool,
    pub bank_count: u32,
    pub misc_slots: u32,
    pub shops: HashMap<ShopId, Arc<Shop>>,
    pub(crate) last_shop: Option<ShopId>,
    pub(crate) buying: Option<ShopId>,
    pub target: Option<MonMapId>,
    pub cooldown_starts: HashMap<String, Instant>,
    pub rev: u64,
}

#[derive(Clone)]
pub struct Shop {
    pub id: ShopId,
    pub name: String,
    pub items: Vec<ShopItem>,
    pub info: Value,
}

#[derive(Clone)]
pub struct ShopItem {
    pub id: ShopItemId,
    pub item: Item,
    pub cost: u32,
    pub reputation: Option<(FactionId, u32)>,
}

#[derive(Clone)]
pub struct Me {
    pub eid: EntityId,
    pub user: String,
    pub char_id: Option<u32>,
    pub level: u32,
    pub gold: i64,
    pub class: String,
    pub cell: String,
    pub pad: String,
    pub x: i32,
    pub y: i32,
    pub afk: bool,
    pub vitals: Vitals,
    pub auras: Vec<Aura>,
    pub skills: Vec<Skill>,
    pub stats: BTreeMap<String, f64>,
    pub member: bool,
    pub aq_linked: bool,
    pub email_verified: bool,
    pub coins: i64,
    pub bag_slots: u32,
    pub bank_slots: u32,
    pub house_slots: u32,
    pub item_skill: Option<u32>,
    pub quest_slots: Vec<String>,
    pub achievements: HashMap<String, i64>,
    pub class_points: u32,
    pub boosts: HashMap<Boost, Instant>,
    pub left_combat: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Boost {
    Class,
    Experience,
    Gold,
    Reputation,
}

impl Boost {
    const ALL: [Boost; 4] = [Boost::Class, Boost::Experience, Boost::Gold, Boost::Reputation];

    fn user_data_key(self) -> &'static str {
        match self {
            Boost::Class => "iBoostCP",
            Boost::Experience => "iBoostXP",
            Boost::Gold => "iBoostG",
            Boost::Reputation => "iBoostRep",
        }
    }

    fn from_cmd(cmd: &str) -> Option<Boost> {
        Boost::ALL.into_iter().find(|b| b.cmd() == cmd)
    }

    fn cmd(self) -> &'static str {
        match self {
            Boost::Class => "cpboost",
            Boost::Experience => "xpboost",
            Boost::Gold => "gboost",
            Boost::Reputation => "repboost",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Vitals {
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
    pub state: State,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    Dead,
    #[default]
    Idle,
    Combat,
}

#[derive(Clone)]
pub struct Area {
    pub id: AreaId,
    pub name: String,
    pub map: String,
    pub file: String,
    pub cells: Vec<String>,
    pub monsters: Vec<Monster>,
    pub players: Vec<Player>,
    announced: bool,
}

#[derive(Clone)]
pub struct Monster {
    pub map_id: MonMapId,
    pub mon_id: MonId,
    pub name: String,
    pub cell: String,
    pub vitals: Vitals,
    pub auras: Vec<Aura>,
    pub counter_warned: Option<Instant>,
    pub aggressive: bool,
}

#[derive(Clone)]
pub struct Player {
    pub eid: EntityId,
    pub name: String,
    pub cell: String,
    pub pad: String,
    pub level: u32,
    pub vitals: Vitals,
    pub auras: Vec<Aura>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemRef {
    Id(ItemId),
    Name(String),
}

impl ItemRef {
    pub fn matches(&self, item: &Item) -> bool {
        match self {
            ItemRef::Id(id) => item.id == *id,
            ItemRef::Name(name) => item.name.eq_ignore_ascii_case(name),
        }
    }
}

impl std::fmt::Display for ItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemRef::Id(id) => write!(f, "item {id}"),
            ItemRef::Name(name) => f.write_str(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: ItemId,
    pub char_item: Option<CharItemId>,
    pub name: String,
    pub qty: u32,
    pub max_stack: u32,
    pub kind: String,
    pub slot: String,
    pub equipped: bool,
    pub ac: bool,
    pub member: bool,
    pub house: bool,
    pub link: String,
    pub file: String,
    pub icon: String,
    pub meta: String,
    pub level: u32,
    pub enhancement: Option<Enhancement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pool {
    Bag,
    Misc,
    Class,
    House,
}

impl Item {
    pub fn pool(&self) -> Pool {
        let numeric_meta = !self.meta.trim().is_empty() && self.meta.trim().bytes().all(|b| b.is_ascii_digit());
        match self.kind.as_str() {
            "Class" => Pool::Class,
            _ if self.house => Pool::House,
            "House" | "Wall Item" | "Floor Item" | "Guild" => Pool::House,
            "Item" if numeric_meta => Pool::Bag,
            "Item" | "Note" | "Quest Item" | "Resource" => Pool::Misc,
            _ => Pool::Bag,
        }
    }

    pub fn consumable(&self) -> bool {
        self.kind == "Item" && !self.link.is_empty() && !self.link.eq_ignore_ascii_case("none")
    }

    pub fn class_rank(&self) -> Option<u32> {
        (self.pool() == Pool::Class).then(|| rank_for(self.qty))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Enhancement {
    pub id: u32,
    pub level: u32,
    pub pattern: u32,
}

#[derive(Clone)]
pub struct Faction {
    pub id: FactionId,
    pub name: String,
    pub rep: u32,
}

impl Faction {
    pub fn rank(&self) -> u32 {
        rank_for(self.rep)
    }
}

#[derive(Clone)]
pub struct Aura {
    pub name: String,
    pub stacks: u32,
    pub until: Option<Instant>,
    pub persist: bool,
}

#[derive(Clone)]
pub struct Skill {
    pub slot: String,
    pub name: String,
    pub cooldown: Duration,
    pub mana: i32,
    pub target: SkillTarget,
    pub max_targets: u32,
    pub locked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillTarget {
    Hostile,
    Friendly,
    Myself,
}

pub fn reduce(world: &mut World, msg: &Message, now: Instant) -> Vec<Event> {
    let mut events = Vec::new();
    world.rev += 1;
    world.expire_auras(now);
    match msg {
        Message::Xml(xml) => world.on_xml(xml, &mut events),
        Message::Xt(xt) => world.on_xt(xt, now, &mut events),
        Message::Json(json) => {
            if let Err(e) = world.on_json(json, now, &mut events) {
                events.push(Event::Unreadable { what: format!("{}: {e}", json.cmd) });
            }
        }
    }
    events
}

#[derive(Deserialize, Default)]
struct Patch {
    #[serde(rename = "intHP")]
    hp: Option<i64>,
    #[serde(rename = "intHPMax")]
    max_hp: Option<i64>,
    #[serde(rename = "intMP")]
    mp: Option<i64>,
    #[serde(rename = "intMPMax")]
    max_mp: Option<i64>,
    #[serde(rename = "intState")]
    state: Option<u8>,
    #[serde(rename = "strFrame")]
    cell: Option<String>,
    #[serde(rename = "strPad")]
    pad: Option<String>,
}

impl Patch {
    fn place(&self, vitals: &mut Vitals, cell: &mut String, pad: &mut String) {
        vitals.apply(self);
        if let Some(c) = &self.cell {
            cell.clone_from(c);
        }
        if let Some(p) = &self.pad {
            pad.clone_from(p);
        }
    }
}

impl Monster {
    pub fn alive(&self) -> bool {
        self.vitals.state != State::Dead
    }

    pub fn countering(&self, now: Instant) -> bool {
        self.alive()
            && (self.counter_warned.is_some_and(|at| now < at + COUNTER_WINDOW)
                || self.auras.iter().any(|a| a.name == COUNTER_AURA && a.until.is_none_or(|until| now < until)))
    }

    fn apply(&mut self, patch: &Patch) {
        self.vitals.apply(patch);
        if self.vitals.state == State::Dead {
            self.auras.clear();
            self.counter_warned = None;
        }
    }
}

impl Vitals {
    fn apply(&mut self, p: &Patch) {
        for (value, field) in [(p.hp, &mut self.hp), (p.max_hp, &mut self.max_hp), (p.mp, &mut self.mp), (p.max_mp, &mut self.max_mp)] {
            if let Some(v) = value {
                *field = qty(v);
            }
        }
        if let Some(s) = p.state {
            self.state = match s {
                0 => State::Dead,
                2 => State::Combat,
                _ => State::Idle,
            };
        }
    }
}

#[derive(Deserialize)]
struct ItemData {
    #[serde(rename = "ItemID")]
    id: Option<ItemId>,
    #[serde(rename = "CharItemID")]
    char_item: Option<CharItemId>,
    #[serde(rename = "sName", default, deserialize_with = "text")]
    name: String,
    #[serde(rename = "iQty", default, deserialize_with = "int")]
    qty: i64,
    #[serde(rename = "iQtyNow")]
    qty_now: Option<i64>,
    #[serde(rename = "iStk", default, deserialize_with = "int")]
    max_stack: i64,
    #[serde(rename = "sType", default, deserialize_with = "text")]
    kind: String,
    #[serde(rename = "sES", default, deserialize_with = "text")]
    slot: String,
    #[serde(rename = "bEquip", default, deserialize_with = "flag")]
    equipped: bool,
    #[serde(rename = "bCoins", default, deserialize_with = "flag")]
    ac: bool,
    #[serde(rename = "bUpg", default, deserialize_with = "flag")]
    member: bool,
    #[serde(rename = "bHouse", default, deserialize_with = "flag")]
    house: bool,
    #[serde(rename = "bTemp", default, deserialize_with = "flag")]
    temp: bool,
    #[serde(rename = "bBank", default, deserialize_with = "flag")]
    bank: bool,
    #[serde(rename = "sLink", default, deserialize_with = "text")]
    link: String,
    #[serde(rename = "sFile", default, deserialize_with = "text")]
    file: String,
    #[serde(rename = "sIcon", default, deserialize_with = "text")]
    icon: String,
    #[serde(rename = "sMeta", default, deserialize_with = "text")]
    meta: String,
    #[serde(rename = "iLvl", default, deserialize_with = "int")]
    level: i64,
    #[serde(rename = "EnhID", default, deserialize_with = "int")]
    enhancement: i64,
    #[serde(rename = "EnhLvl", default, deserialize_with = "int")]
    enhancement_level: i64,
    #[serde(rename = "EnhPatternID", default, deserialize_with = "int")]
    enhancement_pattern: i64,
}

impl ItemData {
    fn keyed(self, key: &str) -> Option<(ItemId, ItemData)> {
        let id = self.id.or_else(|| key.parse().ok().map(ItemId))?;
        Some((id, self))
    }

    fn into_item(self, id: ItemId) -> Item {
        Item {
            id,
            char_item: self.char_item,
            name: self.name,
            qty: qty(self.qty_now.unwrap_or(self.qty)),
            max_stack: qty(self.max_stack),
            kind: self.kind,
            slot: self.slot,
            equipped: self.equipped,
            ac: self.ac,
            member: self.member,
            house: self.house,
            link: self.link,
            file: self.file,
            icon: self.icon,
            meta: self.meta,
            level: qty(self.level),
            enhancement: (self.enhancement > 0).then(|| Enhancement {
                id: qty(self.enhancement),
                level: qty(self.enhancement_level),
                pattern: qty(self.enhancement_pattern),
            }),
        }
    }
}

#[derive(Deserialize)]
struct AuraChange {
    cmd: String,
    #[serde(rename = "tInf", default)]
    target: String,
    #[serde(default)]
    auras: Vec<AuraAdd>,
    aura: Option<AuraName>,
}

#[derive(Deserialize)]
struct AuraAdd {
    nam: String,
    dur: Option<f64>,
    stk: Option<u32>,
    #[serde(default, deserialize_with = "flag")]
    persist: bool,
}

#[derive(Deserialize)]
struct AuraName {
    nam: String,
}

#[derive(Deserialize)]
struct Tick {
    #[serde(default)]
    p: HashMap<String, Patch>,
    #[serde(default)]
    m: HashMap<String, Patch>,
    #[serde(default)]
    a: Vec<AuraChange>,
    #[serde(default)]
    anims: Vec<Anim>,
}

#[derive(Deserialize)]
struct Anim {
    #[serde(rename = "cInf", default, deserialize_with = "text")]
    caster: String,
    #[serde(default, deserialize_with = "text")]
    msg: String,
}

fn rank_for(points: u32) -> u32 {
    1 + RANK_POINTS.iter().filter(|&&threshold| points >= threshold).count() as u32
}

fn yes() -> bool {
    true
}

fn skill_target(tgt: &str) -> SkillTarget {
    match tgt {
        "f" => SkillTarget::Friendly,
        "s" => SkillTarget::Myself,
        _ => SkillTarget::Hostile,
    }
}

fn qty(n: i64) -> u32 {
    n.clamp(0, u32::MAX as i64) as u32
}

fn add_item(list: &mut Vec<Item>, item: Item) -> u32 {
    match list.iter_mut().find(|i| i.id == item.id) {
        Some(existing) => {
            existing.qty += item.qty;
            existing.char_item = item.char_item.or(existing.char_item);
            existing.qty
        }
        None => {
            let total = item.qty;
            list.push(item);
            total
        }
    }
}

pub(crate) fn items_from(values: Vec<Value>, what: &str, events: &mut Vec<Event>) -> Vec<Item> {
    each::<ItemData>(values, what, events).into_iter().filter_map(|d| d.id.map(|id| d.into_item(id))).collect()
}

fn each<T: DeserializeOwned>(values: Vec<Value>, what: &str, events: &mut Vec<Event>) -> Vec<T> {
    let mut skipped = 0;
    let mut first = None;
    let parsed = values
        .into_iter()
        .filter_map(|v| {
            serde_json::from_value(v)
                .map_err(|e| {
                    skipped += 1;
                    first.get_or_insert(e.to_string());
                })
                .ok()
        })
        .collect();
    if let Some(error) = first {
        events.push(Event::Unreadable { what: format!("skipped {skipped} {what} entries, first: {error}") });
    }
    parsed
}

impl World {
    pub fn recharged(&self, skill: &Skill, now: Instant) -> f64 {
        let (Some(me), Some(start)) = (&self.me, self.cooldown_starts.get(&skill.slot)) else { return 1.0 };
        let cooldown = me.cooldown(skill);
        if cooldown.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(*start).as_secs_f64() / cooldown.as_secs_f64()).min(1.0)
    }

    fn on_xml(&mut self, xml: &Xml, events: &mut Vec<Event>) {
        let me = self.me.as_ref().map(|m| m.eid);
        match xml.action {
            Some("joinOK") => {
                let Some(id) = xml.room() else { return };
                let players = xml.users().filter(|(eid, _)| Some(*eid) != me).map(|(eid, name)| Player::new(eid, name)).collect();
                self.area = Some(Area { id, name: String::new(), map: String::new(), file: String::new(), cells: Vec::new(), monsters: Vec::new(), players, announced: false });
            }
            Some("uER") => {
                if let Some(area) = &mut self.area {
                    for (eid, name) in xml.users() {
                        if Some(eid) != me && !area.players.iter().any(|p| p.eid == eid) {
                            area.players.push(Player::new(eid, name));
                            events.push(Event::PlayerEntered { eid });
                        }
                    }
                }
            }
            Some("userGone") => {
                if let Some(area) = &mut self.area {
                    let gone: Vec<_> = xml.user_ids().collect();
                    area.players.retain(|p| !gone.contains(&p.eid));
                }
            }
            _ => {}
        }
    }

    fn on_xt(&mut self, xt: &Xt, now: Instant, events: &mut Vec<Event>) {
        match xt.cmd {
            "loginResponse" => match (xt.arg(0), xt.arg(1).and_then(|s| s.parse().ok())) {
                (Some("true"), Some(eid)) => {
                    let eid = EntityId(eid);
                    self.me = Some(Me::new(eid, xt.arg(2).unwrap_or_default()));
                    events.push(Event::LoggedIn { eid });
                }
                _ => {
                    let reason = xt.args().filter(|a| !a.is_empty()).last().unwrap_or("no reason given");
                    events.push(Event::LoginRejected { reason: reason.into() });
                }
            },
            "server" => {
                let text = xt.arg(0).unwrap_or_default();
                match &mut self.area {
                    Some(area) if text.starts_with("You joined \"") && !area.announced => {
                        area.announced = true;
                        events.push(Event::Joined { area: area.name.clone(), players: area.players.len() + 1 });
                    }
                    Some(_) if text.starts_with("You joined \"") => {}
                    _ => events.push(Event::ServerText { text: text.into() }),
                }
            }
            "warning" => events.push(Event::Warning { text: xt.arg(0).unwrap_or_default().into() }),
            "notify" | "logoutWarning" => events.push(Event::ServerText { text: xt.arg(0).unwrap_or_default().into() }),
            "multiLoginWarning" => events.push(Event::LoggedInElsewhere),
            "zm" if matches!(xt.room, "tryQuestComplete" | "acceptQuest") => {
                if let Some(id) = xt.arg(1).and_then(|s| s.parse().ok()).map(QuestId) {
                    events.push(Event::QuestRejected { id, reason: "the server refused it".into() });
                }
            }
            "chatm" => {
                let payload = xt.arg(0).unwrap_or_default();
                let (channel, text) = payload.split_once('~').unwrap_or(("zone", payload));
                let sender = xt.arg(1).unwrap_or_default();
                events.push(Event::Chat { channel: channel.into(), sender: sender.into(), text: text.into() });
            }
            "whisper" => events.push(Event::Chat {
                channel: "whisper".into(),
                sender: xt.arg(1).unwrap_or_default().into(),
                text: xt.arg(0).unwrap_or_default().into(),
            }),
            "exitArea" => {
                if let (Some(area), Some(eid)) = (&mut self.area, xt.arg(0).and_then(|s| s.parse().ok())) {
                    area.players.retain(|p| p.eid != EntityId(eid));
                }
            }
            "uotls" => {
                let (Some(name), Some(fields)) = (xt.arg(0), xt.arg(1)) else { return };
                let mut patch = Patch::default();
                let (mut afk, mut x, mut y) = (None, None, None);
                for (key, value) in fields.split(',').filter_map(|kv| kv.split_once(':')) {
                    match key {
                        "strFrame" => patch.cell = Some(value.into()),
                        "strPad" => patch.pad = Some(value.into()),
                        "afk" => afk = Some(value == "true"),
                        "tx" => x = value.parse().ok(),
                        "ty" => y = value.parse().ok(),
                        _ => {}
                    }
                }
                if let Some(me) = self.me.as_mut().filter(|m| m.user.eq_ignore_ascii_case(name)) {
                    if let Some(afk) = afk.filter(|afk| *afk != me.afk) {
                        me.afk = afk;
                        events.push(Event::Afk { afk });
                    }
                    me.x = x.unwrap_or(me.x);
                    me.y = y.unwrap_or(me.y);
                }
                self.patch_named(name, &patch, now, events);
            }
            _ => {}
        }
    }

    fn on_json(&mut self, json: &Json, now: Instant, events: &mut Vec<Event>) -> serde_json::Result<()> {
        match json.cmd {
            "moveToArea" => self.move_to_area(json.decode()?, events),
            "initUserData" => {
                let init: Init = json.decode()?;
                self.user_data(init.uid, init.data, now);
            }
            "initUserDatas" => {
                #[derive(Deserialize)]
                struct Inits {
                    a: Vec<Init>,
                }
                for init in json.decode::<Inits>()?.a {
                    self.user_data(init.uid, init.data, now);
                }
            }
            "uotls" => {
                #[derive(Deserialize)]
                struct Update {
                    unm: String,
                    o: Patch,
                }
                let update: Update = json.decode()?;
                self.patch_named(&update.unm, &update.o, now, events);
            }
            "mtls" => {
                #[derive(Deserialize)]
                struct Update {
                    id: MonMapId,
                    o: Patch,
                }
                let update: Update = json.decode()?;
                if let Some(m) = self.monster(update.id) {
                    m.apply(&update.o);
                }
            }
            "ct" | "cb" => {
                let tick: Tick = json.decode()?;
                for (name, patch) in &tick.p {
                    self.patch_named(name, patch, now, events);
                }
                for (id, patch) in &tick.m {
                    if let Some(m) = id.parse().ok().and_then(|id| self.monster(MonMapId(id))) {
                        m.apply(patch);
                    }
                }
                for change in tick.a {
                    self.aura(change, now, events);
                }
                for anim in tick.anims.iter().filter(|a| a.msg.contains(COUNTER_WARNING)) {
                    if let Some(m) = self.monster_ref(&anim.caster) {
                        if m.counter_warned.is_none() {
                            events.push(Event::Countering { id: m.map_id, active: true });
                        }
                        m.counter_warned = Some(now);
                    }
                }
            }
            "aura+" | "aura++" | "aura-" | "aura--" => self.aura(json.decode()?, now, events),
            "clearAuras" => {
                if let Some(me) = &mut self.me {
                    me.auras.retain(|a| a.persist);
                }
            }
            "loadInventoryBig" => {
                #[derive(Deserialize)]
                struct Inventory {
                    #[serde(default)]
                    items: Vec<Value>,
                    #[serde(default)]
                    hitems: Vec<Value>,
                    #[serde(default)]
                    factions: Vec<Value>,
                    #[serde(rename = "bankCount", default, deserialize_with = "int")]
                    bank_count: i64,
                    #[serde(rename = "iMiscSlots", default = "default_misc_slots", deserialize_with = "int")]
                    misc_slots: i64,
                }
                fn default_misc_slots() -> i64 {
                    DEFAULT_MISC_SLOTS
                }
                #[derive(Deserialize)]
                struct FactionData {
                    #[serde(rename = "FactionID")]
                    id: FactionId,
                    #[serde(rename = "sName", default, deserialize_with = "text")]
                    name: String,
                    #[serde(rename = "iRep", default, deserialize_with = "int")]
                    rep: i64,
                }
                let inventory: Inventory = json.decode()?;
                self.inventory = items_from(inventory.items, "inventory item", events);
                self.house = items_from(inventory.hitems, "house item", events);
                if !self.bank_loaded {
                    self.bank_count = qty(inventory.bank_count);
                }
                self.misc_slots = qty(inventory.misc_slots);
                self.factions = each::<FactionData>(inventory.factions, "faction", events)
                    .into_iter()
                    .map(|f| Faction { id: f.id, name: f.name, rep: qty(f.rep) })
                    .collect();
                events.push(Event::InventoryLoaded { items: self.inventory.len() });
            }
            "addItems" => {
                #[derive(Deserialize)]
                struct Added {
                    items: HashMap<String, ItemData>,
                }
                for (key, data) in json.decode::<Added>()?.items {
                    let Some((id, data)) = data.keyed(&key) else { continue };
                    if data.bank {
                        let now = data.qty_now.map(qty);
                        let item = data.into_item(id);
                        let name = item.name.clone();
                        let total = self.add_to_bank(item, now);
                        events.push(Event::ItemChanged { item: name, qty: total, place: Place::Bank });
                        continue;
                    }
                    let temp = data.temp || data.char_item.is_none();
                    let places: &[Place] = if temp { &[Place::Temp] } else { &[Place::House, Place::Inventory] };
                    let (total, place, name) = match self.find_held(places, |i| i.id == id) {
                        Some((place, at)) => {
                            let item = &mut self.held_in(place)[at];
                            item.qty = qty(data.qty_now.unwrap_or(item.qty as i64 + data.qty));
                            (item.qty, place, item.name.clone())
                        }
                        None if temp => {
                            let item = data.into_item(id);
                            let name = item.name.clone();
                            (add_item(&mut self.temp, item), Place::Temp, name)
                        }
                        None => {
                            let item = data.into_item(id);
                            let name = item.name.clone();
                            let (total, place) = self.stash(item);
                            (total, place, name)
                        }
                    };
                    events.push(Event::ItemChanged { item: name, qty: total, place });
                }
            }
            "dropItem" => {
                #[derive(Deserialize)]
                struct Dropped {
                    items: HashMap<String, ItemData>,
                }
                for (key, data) in json.decode::<Dropped>()?.items {
                    let Some((id, data)) = data.keyed(&key) else { continue };
                    let item = data.into_item(id);
                    events.push(Event::DropOffered { id: item.id, item: item.name.clone(), qty: item.qty });
                    match self.drops.iter_mut().find(|d| d.id == item.id) {
                        Some(d) => d.qty += item.qty,
                        None => self.drops.push(item),
                    }
                }
            }
            "getDrop" => {
                #[derive(Deserialize)]
                struct Picked {
                    #[serde(rename = "bSuccess", deserialize_with = "flag")]
                    ok: bool,
                    #[serde(rename = "ItemID")]
                    id: ItemId,
                    #[serde(rename = "CharItemID")]
                    char_item: Option<CharItemId>,
                    #[serde(rename = "iQtyNow", default, deserialize_with = "int")]
                    qty_now: i64,
                    #[serde(rename = "bBank", default, deserialize_with = "flag")]
                    bank: bool,
                }
                let picked: Picked = json.decode()?;
                let Some(at) = self.drops.iter().position(|d| d.id == picked.id).filter(|_| picked.ok) else { return Ok(()) };
                let mut item = self.drops.remove(at);
                item.qty = qty(picked.qty_now);
                item.char_item = picked.char_item.or(item.char_item);
                let name = item.name.clone();
                if picked.bank {
                    let total = self.add_to_bank(item, Some(qty(picked.qty_now)));
                    events.push(Event::ItemChanged { item: name, qty: total, place: Place::Bank });
                } else {
                    events.push(Event::ItemChanged { item: name, qty: item.qty, place: Place::Inventory });
                    match self.inventory.iter_mut().find(|i| i.id == item.id) {
                        Some(existing) => existing.qty = item.qty,
                        None => self.inventory.push(item),
                    }
                }
            }
            "turnIn" => {
                #[derive(Deserialize)]
                struct TurnIn {
                    #[serde(rename = "sItems")]
                    items: String,
                }
                let mut used_up = Vec::new();
                for (id, n) in json.decode::<TurnIn>()?.items.split(',').filter_map(|pair| pair.split_once(':')) {
                    let (Ok(id), Ok(n)) = (id.parse().map(ItemId), n.parse::<u32>()) else { continue };
                    for (list, place) in [(&mut self.inventory, Place::Inventory), (&mut self.temp, Place::Temp)] {
                        if let Some(at) = list.iter().position(|i| i.id == id) {
                            list[at].qty = list[at].qty.saturating_sub(n);
                            events.push(Event::ItemChanged { item: list[at].name.clone(), qty: list[at].qty, place });
                            if list[at].qty == 0 {
                                used_up.push(list.remove(at));
                            }
                            break;
                        }
                    }
                }
                for item in &used_up {
                    self.forget_consumable(item);
                }
            }
            "addGoldExp" => {
                #[derive(Deserialize)]
                struct Reward {
                    #[serde(rename = "intGold", default, deserialize_with = "int")]
                    gold: i64,
                    #[serde(rename = "iCP", default, deserialize_with = "int")]
                    class_points: i64,
                    #[serde(default)]
                    typ: String,
                    id: Option<MonMapId>,
                }
                let reward: Reward = json.decode()?;
                if let Some(me) = &mut self.me {
                    me.gold += reward.gold;
                    me.class_points = me.class_points.saturating_add(qty(reward.class_points));
                }
                if let ("m", Some(id)) = (reward.typ.as_str(), reward.id) {
                    let monster = self.monster(id).map_or_else(|| format!("monster {id}"), |m| m.name.clone());
                    events.push(Event::Killed { id, monster });
                }
            }
            "getQuests" => {
                #[derive(Deserialize)]
                struct Loaded {
                    #[serde(default)]
                    quests: HashMap<String, Value>,
                }
                let loaded = json.decode::<Loaded>()?.quests.into_values().collect();
                for data in each::<QuestData>(loaded, "quest", events) {
                    let quest = data.into_quest();
                    self.quests.insert(quest.id, quest);
                }
            }
            "acceptQuest" | "ccqr" => {
                #[derive(Deserialize)]
                struct Outcome {
                    #[serde(rename = "bSuccess", default, deserialize_with = "flag")]
                    ok: bool,
                    #[serde(rename = "QuestID")]
                    id: QuestId,
                    #[serde(default, deserialize_with = "text")]
                    msg: String,
                }
                let Outcome { ok, id, msg } = json.decode()?;
                let name = self.quests.get(&id).map_or_else(|| format!("quest {id}"), |q| q.name.clone());
                match (json.cmd, ok) {
                    ("acceptQuest", true) => {
                        if !self.active.contains(&id) {
                            self.active.push(id);
                        }
                        events.push(Event::QuestAccepted { id, name });
                    }
                    (_, true) => {
                        self.active.retain(|q| *q != id);
                        events.push(Event::QuestCompleted { id, name });
                    }
                    (_, false) => {
                        let reason = if msg.is_empty() { "the server refused it".into() } else { msg };
                        events.push(Event::QuestRejected { id, reason });
                    }
                }
            }
            "updateQuest" => {
                #[derive(Deserialize)]
                struct Update {
                    #[serde(rename = "iIndex", deserialize_with = "int")]
                    slot: i64,
                    #[serde(rename = "iValue", deserialize_with = "int")]
                    value: i64,
                }
                let update: Update = json.decode()?;
                if let Some(me) = &mut self.me {
                    set_quest_value(me, update.slot as i32, update.value.max(0) as u32);
                }
            }
            "setAchievement" => {
                #[derive(Deserialize)]
                struct Achievement {
                    #[serde(deserialize_with = "text")]
                    field: String,
                    #[serde(deserialize_with = "int")]
                    index: i64,
                    #[serde(deserialize_with = "int")]
                    value: i64,
                }
                let a: Achievement = json.decode()?;
                if let Some(me) = &mut self.me
                    && (0..64).contains(&a.index)
                {
                    let bits = me.achievements.entry(a.field).or_insert(0);
                    if a.value != 0 {
                        *bits |= 1 << a.index;
                    } else {
                        *bits &= !(1 << a.index);
                    }
                }
            }
            "loadShop" => {
                #[derive(Deserialize)]
                struct Loaded {
                    shopinfo: Value,
                }
                #[derive(Deserialize)]
                struct ShopInfo {
                    #[serde(rename = "ShopID")]
                    id: ShopId,
                    #[serde(rename = "sName", default, deserialize_with = "text")]
                    name: String,
                    #[serde(default)]
                    items: Vec<Value>,
                }
                #[derive(Deserialize)]
                struct ShopItemData {
                    #[serde(rename = "ShopItemID")]
                    id: ShopItemId,
                    #[serde(rename = "iCost", default, deserialize_with = "int")]
                    cost: i64,
                    #[serde(rename = "FactionID", default, deserialize_with = "int")]
                    faction: i64,
                    #[serde(rename = "iReqRep", default, deserialize_with = "int")]
                    rep: i64,
                    #[serde(flatten)]
                    item: ItemData,
                }
                let raw = json.decode::<Loaded>()?.shopinfo;
                let info = ShopInfo::deserialize(&raw)?;
                let items = each::<ShopItemData>(info.items, "shop item", events)
                    .into_iter()
                    .filter_map(|s| {
                        let id = s.item.id?;
                        Some(ShopItem {
                            id: s.id,
                            item: s.item.into_item(id),
                            cost: qty(s.cost),
                            reputation: (s.faction > 1 && s.rep > 0).then_some((FactionId(s.faction as u32), qty(s.rep))),
                        })
                    })
                    .collect();
                self.last_shop = Some(info.id);
                self.shops.insert(info.id, Arc::new(Shop { id: info.id, name: info.name, items, info: raw }));
            }
            "buyItem" => {
                #[derive(Deserialize)]
                struct Bought {
                    #[serde(rename = "bitSuccess", default, deserialize_with = "flag")]
                    ok: bool,
                    #[serde(rename = "ItemID")]
                    id: Option<ItemId>,
                    #[serde(rename = "CharItemID")]
                    char_item: Option<CharItemId>,
                    #[serde(rename = "iQty", default, deserialize_with = "int")]
                    qty: i64,
                    #[serde(rename = "bBank", default, deserialize_with = "flag")]
                    bank: bool,
                    #[serde(rename = "strMessage", default, deserialize_with = "text")]
                    message: String,
                }
                let bought: Bought = json.decode()?;
                let from = self.buying.take().or(self.last_shop);
                let listed = bought.id.and_then(|id| {
                    let shops = from.and_then(|s| self.shops.get(&s)).into_iter().chain(self.shops.values());
                    shops.flat_map(|s| &s.items).find(|s| s.item.id == id).cloned()
                });
                match (bought.ok, listed) {
                    (true, Some(listed)) => {
                        let n = qty(bought.qty);
                        if let Some(me) = &mut self.me {
                            let price = i64::from(listed.cost) * i64::from(n);
                            if listed.item.ac { me.coins -= price } else { me.gold -= price }
                        }
                        let mut item = listed.item;
                        item.qty = n;
                        item.char_item = bought.char_item;
                        let name = item.name.clone();
                        let (total, place) =
                            if bought.bank { (self.add_to_bank(item, None), Place::Bank) } else { self.stash(item) };
                        events.push(Event::ItemChanged { item: name, qty: total, place });
                    }
                    (true, None) => {}
                    (false, _) => {
                        let reason = if bought.message.is_empty() { "the server refused it".into() } else { bought.message };
                        events.push(Event::Refused { action: "buying".into(), reason });
                    }
                }
            }
            "bankToInv" | "bankFromInv" => {
                #[derive(Deserialize)]
                struct Moved {
                    #[serde(rename = "ItemID")]
                    id: ItemId,
                    #[serde(rename = "bSuccess", default = "yes", deserialize_with = "flag")]
                    ok: bool,
                    #[serde(default, deserialize_with = "text")]
                    msg: String,
                }
                let moved: Moved = json.decode()?;
                let withdrawing = json.cmd == "bankToInv";
                if !moved.ok {
                    let reason = if moved.msg.is_empty() { "the server refused it".into() } else { moved.msg };
                    events.push(Event::Refused { action: if withdrawing { "withdrawing" } else { "depositing" }.into(), reason });
                } else if withdrawing {
                    let bank = Arc::make_mut(&mut self.bank);
                    if let Some(at) = bank.iter().position(|i| i.id == moved.id) {
                        let item = bank.remove(at);
                        if !item.ac {
                            self.bank_count = self.bank_count.saturating_sub(1);
                        }
                        let name = item.name.clone();
                        let (total, place) = self.stash(item);
                        events.push(Event::ItemChanged { item: name, qty: total, place });
                    }
                } else if let Some((place, at)) = self.find_held(&[Place::Inventory, Place::House], |i| i.id == moved.id) {
                    let item = self.held_in(place).remove(at);
                    let name = item.name.clone();
                    let total = self.add_to_bank(item, None);
                    events.push(Event::ItemChanged { item: name, qty: total, place: Place::Bank });
                }
            }
            "enhanceItemShop" => {
                #[derive(Deserialize)]
                struct Enhanced {
                    #[serde(rename = "bSuccess", default, deserialize_with = "flag")]
                    ok: bool,
                    #[serde(rename = "ItemIDs", default)]
                    items: Vec<Value>,
                    #[serde(rename = "EnhID", default, deserialize_with = "int")]
                    id: i64,
                    #[serde(rename = "EnhLvl", default, deserialize_with = "int")]
                    level: i64,
                    #[serde(rename = "EnhPID", default, deserialize_with = "int")]
                    pattern: i64,
                    #[serde(rename = "iCost", default, deserialize_with = "int")]
                    cost: i64,
                }
                let enhanced: Enhanced = json.decode()?;
                if !enhanced.ok {
                    events.push(Event::Refused { action: "enhancing".into(), reason: "the server refused it".into() });
                    return Ok(());
                }
                let ids: Vec<u64> = enhanced.items.iter().filter_map(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())).collect();
                let enhancement = Enhancement { id: qty(enhanced.id), level: qty(enhanced.level), pattern: qty(enhanced.pattern) };
                for item in self.inventory.iter_mut().filter(|i| ids.contains(&u64::from(i.id.0))) {
                    item.enhancement = Some(enhancement);
                }
                if let Some(me) = &mut self.me {
                    me.gold -= enhanced.cost;
                }
            }
            "seia" => {
                #[derive(Deserialize)]
                struct Equipped {
                    o: Skill5,
                    #[serde(rename = "iRes", default = "yes", deserialize_with = "flag")]
                    ok: bool,
                }
                #[derive(Deserialize)]
                struct Skill5 {
                    #[serde(deserialize_with = "int")]
                    id: i64,
                    #[serde(default, deserialize_with = "int")]
                    cd: i64,
                    #[serde(default, deserialize_with = "int")]
                    mp: i64,
                    #[serde(default, deserialize_with = "text")]
                    tgt: String,
                    #[serde(rename = "tgtMax", default, deserialize_with = "int")]
                    max_targets: i64,
                }
                let equipped: Equipped = json.decode()?;
                if !equipped.ok {
                    events.push(Event::Refused { action: "equipping".into(), reason: "the server refused it".into() });
                } else if let Some(me) = &mut self.me {
                    let o = equipped.o;
                    me.item_skill = Some(qty(o.id));
                    for item in self.inventory.iter_mut().filter(|i| i.consumable()) {
                        item.equipped = item.meta.parse() == Ok(qty(o.id));
                    }
                    if let Some(slot) = me.skills.iter_mut().find(|s| s.slot == "i1") {
                        slot.cooldown = Duration::from_millis(o.cd.max(0) as u64);
                        slot.mana = o.mp as i32;
                        slot.target = skill_target(&o.tgt);
                        slot.max_targets = qty(o.max_targets).max(1);
                    }
                }
            }
            "sellItem" => {
                #[derive(Deserialize)]
                struct Sold {
                    #[serde(rename = "CharItemID")]
                    char_item: CharItemId,
                    #[serde(rename = "iQty", default, deserialize_with = "int")]
                    qty: i64,
                    #[serde(rename = "intAmount", default, deserialize_with = "int")]
                    amount: i64,
                    #[serde(rename = "bCoins", default, deserialize_with = "flag")]
                    ac: bool,
                }
                let sold: Sold = json.decode()?;
                if let Some(me) = &mut self.me {
                    if sold.ac {
                        me.coins += sold.amount;
                    } else if me.gold < GOLD_CAP {
                        me.gold += sold.amount;
                    }
                }
                if let Some((place, at)) = self.find_held(&[Place::Inventory, Place::House], |i| i.char_item == Some(sold.char_item)) {
                    let list = self.held_in(place);
                    let item = &mut list[at];
                    item.qty = item.qty.saturating_sub(qty(sold.qty));
                    events.push(Event::ItemChanged { item: item.name.clone(), qty: item.qty, place });
                    if item.qty == 0 {
                        let gone = list.remove(at);
                        self.forget_consumable(&gone);
                    }
                }
            }
            "balance" => {
                #[derive(Deserialize)]
                struct Balance {
                    #[serde(rename = "intGold", deserialize_with = "int")]
                    gold: i64,
                    #[serde(rename = "intCoins")]
                    coins: Option<i64>,
                }
                let balance: Balance = json.decode()?;
                if let Some(me) = &mut self.me {
                    me.gold = balance.gold;
                    me.coins = balance.coins.unwrap_or(me.coins);
                }
            }
            "sAct" => {
                #[derive(Deserialize)]
                struct Actions {
                    actions: Active,
                }
                #[derive(Deserialize)]
                struct Active {
                    #[serde(default)]
                    active: Vec<SkillData>,
                }
                #[derive(Deserialize)]
                struct SkillData {
                    #[serde(rename = "ref")]
                    slot: String,
                    nam: String,
                    #[serde(default, deserialize_with = "int")]
                    cd: i64,
                    #[serde(default, deserialize_with = "int")]
                    mp: i64,
                    #[serde(default, deserialize_with = "text")]
                    tgt: String,
                    #[serde(rename = "tgtMax", default, deserialize_with = "int")]
                    max_targets: i64,
                    #[serde(rename = "isOK", default = "yes", deserialize_with = "flag")]
                    ok: bool,
                }
                let skills = json.decode::<Actions>()?.actions.active;
                if let Some(me) = &mut self.me {
                    me.skills = skills
                        .into_iter()
                        .map(|s| Skill {
                            slot: s.slot,
                            name: s.nam,
                            cooldown: Duration::from_millis(s.cd.max(0) as u64),
                            mana: s.mp as i32,
                            target: skill_target(&s.tgt),
                            max_targets: qty(s.max_targets).max(1),
                            locked: !s.ok,
                        })
                        .collect();
                }
            }
            "stu" => {
                #[derive(Deserialize)]
                struct Stats {
                    #[serde(default)]
                    sta: HashMap<String, serde_json::Value>,
                }
                let stats = json.decode::<Stats>()?.sta;
                if let Some(me) = &mut self.me {
                    me.stats.extend(stats.into_iter().filter_map(|(k, v)| Some((k, v.as_f64()?))));
                }
            }
            "updateClass" => {
                #[derive(Deserialize)]
                struct Class {
                    uid: EntityId,
                    #[serde(rename = "sClassName")]
                    name: String,
                    #[serde(rename = "iCP", default, deserialize_with = "int")]
                    class_points: i64,
                }
                let class: Class = json.decode()?;
                if let Some(me) = self.me.as_mut().filter(|m| m.eid == class.uid) {
                    me.class = class.name;
                    me.class_points = qty(class.class_points);
                }
            }
            "equipItem" | "unequipItem" => {
                #[derive(Deserialize)]
                struct Equip {
                    uid: EntityId,
                    #[serde(rename = "ItemID")]
                    id: ItemId,
                }
                let equip: Equip = json.decode()?;
                if self.me.as_ref().is_some_and(|m| m.eid == equip.uid) {
                    let on = json.cmd == "equipItem";
                    let slot = self.inventory.iter().find(|i| i.id == equip.id).map(|i| i.slot.clone());
                    for item in &mut self.inventory {
                        if item.id == equip.id {
                            item.equipped = on;
                        } else if on && Some(&item.slot) == slot.as_ref() {
                            item.equipped = false;
                        }
                    }
                }
            }
            cmd => {
                if let Some(boost) = Boost::from_cmd(cmd) {
                    #[derive(Deserialize)]
                    struct Boosted {
                        #[serde(default, deserialize_with = "text")]
                        op: String,
                        #[serde(rename = "iSecsLeft", default, deserialize_with = "int")]
                        secs: i64,
                    }
                    let boosted: Boosted = json.decode()?;
                    if let Some(me) = &mut self.me {
                        me.set_boost(boost, if boosted.op == "-" { 0 } else { boosted.secs }, now);
                    }
                }
            }
        }
        Ok(())
    }

    fn move_to_area(&mut self, m: MoveToArea, events: &mut Vec<Event>) {
        let names: HashMap<MonId, String> = each::<MonDef>(m.defs, "monster type", events).into_iter().map(|d| (d.id, d.name)).collect();
        let mut states: HashMap<MonMapId, Patch> =
            each::<MonState>(m.states, "monster state", events).into_iter().map(|s| (s.map_id, s.patch)).collect();
        let monsters = each::<MonSpot>(m.spots, "monster", events)
            .into_iter()
            .map(|spot| {
                let mut vitals = Vitals::default();
                if let Some(patch) = states.remove(&spot.map_id) {
                    vitals.apply(&patch);
                }
                Monster {
                    map_id: spot.map_id,
                    mon_id: spot.mon_id,
                    name: names.get(&spot.mon_id).cloned().unwrap_or_default(),
                    cell: spot.cell,
                    vitals,
                    auras: Vec::new(),
                    counter_warned: None,
                    aggressive: spot.aggressive,
                }
            })
            .collect();
        let players = match self.area.take() {
            Some(area) if area.id == m.id => area.players,
            _ => Vec::new(),
        };
        self.area = Some(Area { id: m.id, name: m.name, map: m.map, file: m.file, cells: Vec::new(), monsters, players, announced: false });
        for avatar in each::<Avatar>(m.avatars, "player", events) {
            let me = self.me.as_mut().filter(|me| me.eid == avatar.eid);
            if let Some(me) = me {
                me.apply(&avatar.patch);
                me.x = avatar.tx.map_or(0, |x| x as i32);
                me.y = avatar.ty.map_or(0, |y| y as i32);
                continue;
            }
            let Some(area) = &mut self.area else { continue };
            let at = match area.players.iter().position(|p| p.eid == avatar.eid) {
                Some(at) => at,
                None => {
                    area.players.push(Player::new(avatar.eid, &avatar.name));
                    area.players.len() - 1
                }
            };
            let player = &mut area.players[at];
            player.level = qty(avatar.level);
            player.apply(&avatar.patch);
        }
    }

    fn user_data(&mut self, uid: EntityId, data: UserData, now: Instant) {
        if let Some(me) = self.me.as_mut().filter(|m| m.eid == uid) {
            me.char_id = data.char_id.or(me.char_id);
            me.level = qty(data.level);
            me.gold = data.gold.unwrap_or(me.gold);
            if let Some(class) = data.class {
                me.class = class;
            }
            if let Some(member) = data.member {
                me.member = member.as_i64().unwrap_or(0) > 0 || member.as_bool() == Some(true);
            }
            for (key, value) in data.rest {
                let as_int = || value.as_i64().or_else(|| value.as_str()?.parse().ok());
                match (key.as_str(), as_int()) {
                    ("intCoins", Some(n)) => me.coins = n,
                    ("iBagSlots", Some(n)) => me.bag_slots = qty(n),
                    ("iBankSlots", Some(n)) => me.bank_slots = qty(n),
                    ("iHouseSlots", Some(n)) => me.house_slots = qty(n),
                    ("intAQ", Some(n)) => me.aq_linked = n > 0,
                    ("intActivationFlag", Some(n)) => me.email_verified = n == EMAIL_VERIFIED,
                    (key, Some(secs)) => {
                        if let Some(boost) = Boost::ALL.into_iter().find(|b| b.user_data_key() == key) {
                            me.set_boost(boost, secs, now);
                        }
                    }
                    _ => {}
                }
                if let Some(n) = key.strip_prefix("strQuests") {
                    let index = if n.is_empty() { Some(0) } else { n.parse::<usize>().ok().map(|n| n - 1) };
                    if let (Some(index), Some(slots)) = (index, value.as_str()) {
                        if me.quest_slots.len() <= index {
                            me.quest_slots.resize(index + 1, String::new());
                        }
                        me.quest_slots[index] = slots.to_string();
                    }
                } else if ["ia", "id", "iw", "im"].iter().any(|p| key.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
                    && let Some(bits) = as_int()
                {
                    me.achievements.insert(key, bits);
                }
            }
        } else if let Some(player) = self.area.as_mut().and_then(|a| a.players.iter_mut().find(|p| p.eid == uid)) {
            player.level = qty(data.level);
        }
    }

    fn patch_named(&mut self, name: &str, patch: &Patch, now: Instant, events: &mut Vec<Event>) {
        if let Some(me) = self.me.as_mut().filter(|m| m.user.eq_ignore_ascii_case(name)) {
            let was = me.vitals.state;
            let spot = (me.cell.clone(), me.pad.clone());
            me.apply(patch);
            if (&me.cell, &me.pad) != (&spot.0, &spot.1) {
                (me.x, me.y) = (0, 0);
                events.push(Event::Moved { cell: me.cell.clone(), pad: me.pad.clone() });
            }
            if was == State::Combat && me.vitals.state != State::Combat {
                me.left_combat = Some(now);
            }
            match (was, me.vitals.state) {
                (State::Dead, State::Dead) => {}
                (_, State::Dead) => events.push(Event::Died),
                (State::Dead, _) => events.push(Event::Respawned),
                _ => {}
            }
        } else if let Some(player) = self.area.as_mut().and_then(|a| a.players.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name))) {
            player.apply(patch);
        }
    }

    pub fn set_bank(&mut self, items: Vec<Item>) {
        self.bank_count = items.iter().filter(|i| !i.ac).count() as u32;
        self.bank = Arc::new(items);
        self.bank_loaded = true;
    }

    pub fn loaded_shop(&self) -> Option<&Shop> {
        self.last_shop.and_then(|id| self.shops.get(&id)).map(Arc::as_ref)
    }

    pub fn used(&self, pool: Pool) -> u32 {
        match pool {
            Pool::House => self.house.len() as u32,
            _ => self.inventory.iter().filter(|i| i.pool() == pool).count() as u32,
        }
    }

    pub fn carried(&self) -> impl Iterator<Item = &Item> {
        self.inventory.iter().chain(&self.house)
    }

    fn stash(&mut self, item: Item) -> (u32, Place) {
        let place = if item.pool() == Pool::House { Place::House } else { Place::Inventory };
        (add_item(self.held_in(place), item), place)
    }

    fn held_in(&mut self, place: Place) -> &mut Vec<Item> {
        match place {
            Place::House => &mut self.house,
            Place::Temp => &mut self.temp,
            Place::Inventory => &mut self.inventory,
            Place::Bank => Arc::make_mut(&mut self.bank),
        }
    }

    fn find_held(&mut self, places: &[Place], hit: impl Fn(&Item) -> bool) -> Option<(Place, usize)> {
        places.iter().find_map(|&place| self.held_in(place).iter().position(&hit).map(|at| (place, at)))
    }

    pub fn full_for(&self, item: &Item) -> bool {
        if item.pool() == Pool::Class {
            return false;
        }
        let held = if item.pool() == Pool::House { &self.house } else { &self.inventory };
        let owned = held.iter().find(|i| i.id == item.id);
        if owned.is_some_and(|i| i.qty < i.max_stack) {
            return false;
        }
        match item.pool() {
            Pool::House => self.me.as_ref().is_some_and(|m| m.house_slots > 0 && self.used(Pool::House) >= m.house_slots),
            Pool::Misc => owned.is_none() && self.misc_slots > 0 && self.used(Pool::Misc) >= self.misc_slots,
            _ => self.me.as_ref().is_some_and(|m| m.bag_slots > 0 && self.used(Pool::Bag) >= m.bag_slots),
        }
    }

    pub fn bank_full(&self) -> bool {
        self.me.as_ref().is_some_and(|m| m.bank_slots > 0 && self.bank_count >= m.bank_slots)
    }

    fn add_to_bank(&mut self, mut item: Item, qty_now: Option<u32>) -> u32 {
        let bank = Arc::make_mut(&mut self.bank);
        if let Some(existing) = bank.iter_mut().find(|i| i.id == item.id) {
            existing.qty = qty_now.unwrap_or(existing.qty + item.qty);
            existing.char_item = item.char_item.or(existing.char_item);
            return existing.qty;
        }
        if let Some(n) = qty_now {
            item.qty = n;
        }
        let (total, counts) = (item.qty, !item.ac);
        bank.push(item);
        if counts {
            self.bank_count += 1;
        }
        total
    }

    fn forget_consumable(&mut self, gone: &Item) {
        if let Some(me) = &mut self.me
            && gone.meta.parse::<u32>().is_ok_and(|skill| me.item_skill == Some(skill))
        {
            me.item_skill = None;
        }
    }

    fn expire_auras(&mut self, now: Instant) {
        let live = |a: &Aura| a.until.is_none_or(|until| until > now);
        if let Some(me) = &mut self.me {
            me.auras.retain(live);
        }
        if let Some(area) = &mut self.area {
            area.monsters.iter_mut().for_each(|m| m.auras.retain(live));
            area.players.iter_mut().for_each(|p| p.auras.retain(live));
        }
    }

    fn monster(&mut self, id: MonMapId) -> Option<&mut Monster> {
        self.area.as_mut()?.monsters.iter_mut().find(|m| m.map_id == id)
    }

    fn monster_ref(&mut self, entity: &str) -> Option<&mut Monster> {
        let id = entity.strip_prefix("m:")?.parse().ok()?;
        self.monster(MonMapId(id))
    }

    fn auras_of(&mut self, target: &str) -> Option<&mut Vec<Aura>> {
        if target.starts_with("m:") {
            return self.monster_ref(target).map(|m| &mut m.auras);
        }
        let eid = EntityId(target.strip_prefix("p:")?.parse().ok()?);
        if self.me.as_ref().is_some_and(|m| m.eid == eid) {
            return self.me.as_mut().map(|m| &mut m.auras);
        }
        self.area.as_mut()?.players.iter_mut().find(|p| p.eid == eid).map(|p| &mut p.auras)
    }

    fn aura_owner(&self, target: &str) -> Option<Option<MonMapId>> {
        if let Some(id) = target.strip_prefix("m:") {
            return id.parse().ok().map(|id| Some(MonMapId(id)));
        }
        let eid = EntityId(target.strip_prefix("p:")?.parse().ok()?);
        self.me.as_ref().filter(|m| m.eid == eid).map(|_| None)
    }

    fn aura(&mut self, change: AuraChange, now: Instant, events: &mut Vec<Event>) {
        let owner = self.aura_owner(&change.target);
        let Some(auras) = self.auras_of(&change.target) else { return };
        match change.cmd.as_str() {
            "aura+" | "aura++" => {
                let mut counter = false;
                for add in change.auras {
                    let until = add
                        .dur
                        .filter(|d| *d > 0.0)
                        .and_then(|d| Duration::try_from_secs_f64(d).ok())
                        .and_then(|d| now.checked_add(d));
                    let aura = Aura { name: add.nam, stacks: add.stk.unwrap_or(1), until, persist: add.persist };
                    counter |= aura.name == COUNTER_AURA;
                    if let Some(monster) = owner {
                        events.push(Event::AuraChanged { monster, name: aura.name.clone(), stacks: aura.stacks, added: true });
                    }
                    match auras.iter_mut().find(|a| a.name == aura.name) {
                        Some(existing) => *existing = aura,
                        None => auras.push(aura),
                    }
                }
                if counter
                    && let Some(m) = self.monster_ref(&change.target).filter(|m| m.counter_warned.is_none())
                {
                    m.counter_warned = Some(now);
                    events.push(Event::Countering { id: m.map_id, active: true });
                }
            }
            "aura-" | "aura--" => {
                let gone: Vec<String> = change.auras.into_iter().map(|a| a.nam).chain(change.aura.map(|a| a.nam)).collect();
                for aura in auras.iter().filter(|a| gone.contains(&a.name)) {
                    if let Some(monster) = owner {
                        events.push(Event::AuraChanged { monster, name: aura.name.clone(), stacks: 0, added: false });
                    }
                }
                auras.retain(|a| !gone.contains(&a.name));
                if gone.iter().any(|name| name == COUNTER_AURA)
                    && let Some(m) = self.monster_ref(&change.target)
                {
                    let id = m.map_id;
                    if m.counter_warned.take().is_some() {
                        events.push(Event::Countering { id, active: false });
                    }
                }
            }
            _ => {}
        }
    }
}

impl Me {
    pub fn class_rank(&self) -> u32 {
        rank_for(self.class_points)
    }

    pub fn boost_left(&self, boost: Boost, now: Instant) -> Duration {
        self.boosts.get(&boost).map_or(Duration::ZERO, |until| until.saturating_duration_since(now))
    }

    fn set_boost(&mut self, boost: Boost, secs: i64, now: Instant) {
        if secs > 0 {
            self.boosts.insert(boost, now + Duration::from_secs(secs as u64));
        } else {
            self.boosts.remove(&boost);
        }
    }

    fn new(eid: EntityId, user: &str) -> Self {
        Me {
            eid,
            user: user.into(),
            char_id: None,
            level: 0,
            gold: 0,
            class: String::new(),
            cell: String::new(),
            pad: String::new(),
            x: 0,
            y: 0,
            afk: false,
            vitals: Vitals::default(),
            auras: Vec::new(),
            skills: Vec::new(),
            stats: BTreeMap::new(),
            member: false,
            aq_linked: false,
            email_verified: false,
            coins: 0,
            bag_slots: 0,
            bank_slots: 0,
            house_slots: 0,
            item_skill: None,
            quest_slots: Vec::new(),
            achievements: HashMap::new(),
            class_points: 0,
            boosts: HashMap::new(),
            left_combat: None,
        }
    }

    fn apply(&mut self, patch: &Patch) {
        patch.place(&mut self.vitals, &mut self.cell, &mut self.pad);
    }

    pub fn haste(&self) -> f64 {
        self.stats.get("$tha").copied().unwrap_or(0.0).clamp(-1.0, HASTE_CAP)
    }

    pub fn cooldown(&self, skill: &Skill) -> Duration {
        skill.cooldown.mul_f64(1.0 - self.haste())
    }
}

impl Player {
    fn new(eid: EntityId, name: &str) -> Self {
        Player {
            eid,
            name: name.into(),
            cell: String::new(),
            pad: String::new(),
            level: 0,
            vitals: Vitals::default(),
            auras: Vec::new(),
        }
    }

    fn apply(&mut self, patch: &Patch) {
        patch.place(&mut self.vitals, &mut self.cell, &mut self.pad);
    }
}

#[derive(Deserialize)]
struct Init {
    uid: EntityId,
    data: UserData,
}

#[derive(Deserialize)]
struct UserData {
    #[serde(rename = "CharID")]
    char_id: Option<u32>,
    #[serde(rename = "intLevel", default, deserialize_with = "int")]
    level: i64,
    #[serde(rename = "intGold")]
    gold: Option<i64>,
    #[serde(rename = "strClassName")]
    class: Option<String>,
    #[serde(rename = "iUpg")]
    member: Option<Value>,
    #[serde(flatten)]
    rest: HashMap<String, Value>,
}

#[derive(Deserialize)]
struct MoveToArea {
    #[serde(rename = "areaId")]
    id: AreaId,
    #[serde(rename = "areaName")]
    name: String,
    #[serde(rename = "strMapName")]
    map: String,
    #[serde(rename = "strMapFileName", default)]
    file: String,
    #[serde(rename = "mondef", default)]
    defs: Vec<Value>,
    #[serde(rename = "monmap", default)]
    spots: Vec<Value>,
    #[serde(rename = "monBranch", default)]
    states: Vec<Value>,
    #[serde(rename = "uoBranch", default)]
    avatars: Vec<Value>,
}

#[derive(Deserialize)]
struct MonDef {
    #[serde(rename = "MonID")]
    id: MonId,
    #[serde(rename = "strMonName", default, deserialize_with = "text")]
    name: String,
}

#[derive(Deserialize)]
struct MonSpot {
    #[serde(rename = "MonMapID")]
    map_id: MonMapId,
    #[serde(rename = "MonID")]
    mon_id: MonId,
    #[serde(rename = "bRed", default, deserialize_with = "flag")]
    aggressive: bool,
    #[serde(rename = "strFrame", default, deserialize_with = "text")]
    cell: String,
}

#[derive(Deserialize)]
struct MonState {
    #[serde(rename = "MonMapID")]
    map_id: MonMapId,
    #[serde(flatten)]
    patch: Patch,
}

#[derive(Deserialize)]
struct Avatar {
    #[serde(rename = "entID")]
    eid: EntityId,
    #[serde(rename = "uoName", default, deserialize_with = "text")]
    name: String,
    #[serde(rename = "intLevel", default, deserialize_with = "int")]
    level: i64,
    tx: Option<f64>,
    ty: Option<f64>,
    #[serde(flatten)]
    patch: Patch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(world: &mut World, text: &str) -> Vec<Event> {
        reduce(world, &Message::parse(text).unwrap(), Instant::now())
    }

    fn json(o: &str) -> String {
        format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#)
    }

    fn joined() -> World {
        let mut world = World::default();
        feed(&mut world, "%xt%loginResponse%-1%true%1001%hero%Welcome!%");
        feed(
            &mut world,
            "<msg t='sys'><body action='joinOK' r='5'><uLs r='5'><u i='1001'><n><![CDATA[hero]]></n></u><u i='1002'><n><![CDATA[ally]]></n></u></uLs></body></msg>",
        );
        feed(
            &mut world,
            &json(
                r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
                "mondef":[{"MonID":10,"strMonName":"Mosquito"}],
                "monmap":[{"MonMapID":1,"MonID":10,"strFrame":"Farm"}],
                "monBranch":[{"MonMapID":1,"MonID":10,"intHP":540,"intHPMax":540,"intState":1}],
                "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3830,"intHPMax":3830,"intMP":100,"intMPMax":100,"intState":1},
                            {"entID":1002,"uoName":"ally","intLevel":40,"strFrame":"Farm","strPad":"Left","intHP":900,"intHPMax":1000,"intState":1}]}"#,
            ),
        );
        world
    }

    #[test]
    fn joining_builds_the_area_from_joinok_and_movetoarea() {
        let mut world = joined();
        assert_eq!(feed(&mut world, "%xt%server%-1%You joined \"farm-1\"%"), [Event::Joined { area: "farm-1".into(), players: 2 }]);
        assert_eq!(feed(&mut world, "%xt%server%-1%You joined \"farm-1\"!%"), [], "one join per moveToArea");
        let area = world.area.as_ref().unwrap();
        assert_eq!((area.id, area.map.as_str()), (AreaId(5), "farm"));
        let mosquito = &area.monsters[0];
        assert_eq!((mosquito.name.as_str(), mosquito.cell.as_str(), mosquito.vitals.hp), ("Mosquito", "Farm", 540));
        let ally = &area.players[..];
        assert_eq!(ally.len(), 1);
        assert_eq!((ally[0].name.as_str(), ally[0].cell.as_str(), ally[0].level, ally[0].vitals.hp), ("ally", "Farm", 40, 900));
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.cell.as_str(), me.vitals.hp, me.vitals.max_hp), ("Enter", 3830, 3830));
    }

    #[test]
    fn combat_ticks_move_hp_auras_and_death() {
        let mut world = joined();
        feed(
            &mut world,
            &json(
                r#"{"cmd":"ct","m":{"1":{"intHP":0,"intState":0,"targets":["hero"]}},"p":{"hero":{"intHP":2000,"intState":2}},
                "a":[{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Corruption","dur":20,"stk":2}]},
                     {"cmd":"aura++","tInf":"m:1","auras":[{"nam":"Counter Attack","dur":6}]}]}"#,
            ),
        );
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.vitals.hp, me.vitals.state), (2000, State::Combat));
        assert_eq!((me.auras[0].name.as_str(), me.auras[0].stacks), ("Corruption", 2));
        let mosquito = &world.area.as_ref().unwrap().monsters[0];
        assert_eq!((mosquito.vitals.state, mosquito.auras[0].name.as_str()), (State::Dead, "Counter Attack"));

        feed(&mut world, &json(r#"{"cmd":"ct","a":[{"cmd":"aura--","tInf":"m:1","aura":{"nam":"Counter Attack"}}]}"#));
        assert!(world.area.as_ref().unwrap().monsters[0].auras.is_empty());
        feed(&mut world, &json(r#"{"cmd":"ct","a":[{"cmd":"aura-","tInf":"p:1001","auras":[{"nam":"Corruption"}]}]}"#));
        assert!(world.me.as_ref().unwrap().auras.is_empty());
        assert_eq!(feed(&mut world, &json(r#"{"cmd":"ct","p":{"hero":{"intHP":0,"intState":0}}}"#)), [Event::Died]);
        assert_eq!(feed(&mut world, &json(r#"{"cmd":"ct","p":{"hero":{"intHP":0,"intState":0}}}"#)), []);
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"uotls","unm":"hero","o":{"intHP":3830,"intState":1,"strFrame":"Enter","strPad":"Spawn"}}"#)),
            [Event::Respawned]
        );
        assert_eq!(feed(&mut world, &json(r#"{"cmd":"mtls","id":1,"o":{"intHP":540,"intState":1}}"#)), []);
        assert_eq!(world.area.as_ref().unwrap().monsters[0].vitals.state, State::Idle);
    }

    #[test]
    fn class_points_follow_the_class_and_every_kill() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"updateClass","uid":1001,"sClassName":"Rogue","iCP":890}"#));
        assert_eq!(world.me.as_ref().unwrap().class_rank(), 1);
        feed(&mut world, &json(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"iCP":22,"typ":"m","id":1}"#));
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.class.as_str(), me.class_points, me.class_rank()), ("Rogue", 912, 2));
    }

    #[test]
    fn boosts_run_out_after_the_seconds_the_server_gives() {
        let mut world = joined();
        let now = Instant::now();
        reduce(&mut world, &Message::parse(&json(r#"{"cmd":"initUserData","uid":1001,"data":{"iBoostCP":"1195","iBoostXP":0}}"#)).unwrap(), now);
        reduce(&mut world, &Message::parse(&json(r#"{"cmd":"gboost","op":"+","iSecsLeft":600,"bShowShop":false}"#)).unwrap(), now);
        let me = world.me.as_ref().unwrap();
        assert_eq!(me.boost_left(Boost::Class, now), Duration::from_secs(1195));
        assert_eq!(me.boost_left(Boost::Gold, now + Duration::from_secs(60)), Duration::from_secs(540));
        assert_eq!(me.boost_left(Boost::Experience, now), Duration::ZERO);
        reduce(&mut world, &Message::parse(&json(r#"{"cmd":"cpboost","op":"-"}"#)).unwrap(), now);
        assert_eq!(world.me.as_ref().unwrap().boost_left(Boost::Class, now), Duration::ZERO);
    }

    #[test]
    fn only_bag_items_count_against_the_bag_slots() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"intLevel":10,"iBagSlots":3}}"#));
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadInventoryBig","factions":[],"iMiscSlots":"4","items":[
                {"ItemID":1,"CharItemID":11,"sName":"Sword","iQty":1,"iStk":1,"sType":"Sword"},
                {"ItemID":2,"CharItemID":12,"sName":"Health Potion","iQty":5,"iStk":10,"sType":"Item","sMeta":"457"},
                {"ItemID":3,"CharItemID":13,"sName":"Junk","iQty":1,"iStk":1,"sType":"Misc"},
                {"ItemID":4,"CharItemID":14,"sName":"Rogue","iQty":21980,"iStk":1,"sType":"Class"},
                {"ItemID":5,"CharItemID":15,"sName":"Bone Dust","iQty":3,"iStk":50,"sType":"Resource"},
                {"ItemID":6,"CharItemID":16,"sName":"Defeated Makai","iQty":50,"iStk":50,"sType":"Quest Item"},
                {"ItemID":7,"CharItemID":17,"sName":"Voucher","iQty":1,"iStk":1,"sType":"Item"},
                {"ItemID":8,"CharItemID":18,"sName":"Letter","iQty":1,"iStk":1,"sType":"Note"}],"hitems":[
                {"ItemID":9,"CharItemID":19,"sName":"Chair","iQty":1,"iStk":1,"sType":"Floor Item","bHouse":true},
                {"ItemID":10,"CharItemID":20,"sName":"Tent","iQty":1,"iStk":1,"sType":"House","bHouse":true}]}"#,
            ),
        );
        let used = |pool| world.used(pool);
        assert_eq!([Pool::Bag, Pool::Misc, Pool::Class, Pool::House].map(used), [3, 4, 1, 2]);
        assert_eq!(world.misc_slots, 4);

        let owned = |name: &str| world.inventory.iter().find(|i| i.name == name).unwrap().clone();
        let new = |kind: &str| Item { id: ItemId(99), kind: kind.into(), ..owned("Sword") };
        assert!(world.full_for(&new("Axe")) && world.full_for(&new("Resource")), "both pools are at their limit");
        assert!(!world.full_for(&owned("Health Potion")) && !world.full_for(&owned("Bone Dust")), "a stack with room takes more");
        assert!(!world.full_for(&new("Class")), "classes have no limit");
        assert_eq!((owned("Rogue").class_rank(), owned("Sword").class_rank()), (Some(4), None), "a class item's quantity is its class points");

        feed(&mut world, &json(r#"{"cmd":"loadInventoryBig","factions":[],"items":[]}"#));
        assert_eq!(world.misc_slots, 100, "the client allows 100 misc items when the server names no limit");
    }

    #[test]
    fn items_follow_drops_pickups_and_turn_ins() {
        let mut world = joined();
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadInventoryBig","items":[{"ItemID":1528,"CharItemID":77,"sName":"Defeated Makai","iQty":3,"iStk":50,"sType":"Quest Item","sES":"None"},
                {"ItemID":15652,"CharItemID":78,"sName":"Rogue","iQty":1,"iStk":1,"sType":"Class","sES":"ar","EnhID":3719,"EnhLvl":22,"EnhPatternID":2}],
                "factions":[{"CharFactionID":"9","FactionID":"46","sName":"Aegis","iRep":"44100"}]}"#,
            ),
        );
        assert_eq!(world.factions[0].rank(), 6);
        let enhancements: Vec<_> = world.inventory.iter().map(|i| i.enhancement).collect();
        assert_eq!(enhancements, [None, Some(Enhancement { id: 3719, level: 22, pattern: 2 })]);
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"addItems","items":{"87":{"ItemID":87,"sName":"Treeant Branch","bTemp":true,"iQty":1}}}"#)),
            [Event::ItemChanged { item: "Treeant Branch".into(), qty: 1, place: Place::Temp }]
        );
        feed(&mut world, &json(r#"{"cmd":"addItems","items":{"87":{"ItemID":87,"sName":"Treeant Branch","bTemp":true,"iQty":1}}}"#));
        assert_eq!(world.temp[0].qty, 2);
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"addItems","items":{"1528":{"iQty":1,"iQtyNow":4,"CharItemID":77,"bBank":0}}}"#)),
            [Event::ItemChanged { item: "Defeated Makai".into(), qty: 4, place: Place::Inventory }]
        );
        feed(&mut world, &json(r#"{"cmd":"turnIn","sItems":"1528:1"}"#));

        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"dropItem","items":{"1528":{"ItemID":1528,"sName":"Defeated Makai","iQty":1}}}"#)),
            [Event::DropOffered { id: ItemId(1528), item: "Defeated Makai".into(), qty: 1 }]
        );
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"getDrop","bSuccess":1,"ItemID":1528,"CharItemID":77,"iQty":1,"iQtyNow":4,"bBank":0}"#)),
            [Event::ItemChanged { item: "Defeated Makai".into(), qty: 4, place: Place::Inventory }]
        );
        assert!(world.drops.is_empty());
        assert_eq!(world.inventory[0].qty, 4);

        feed(&mut world, &json(r#"{"cmd":"turnIn","sItems":"87:2,1528:1"}"#));
        assert!(world.temp.is_empty());
        assert_eq!(world.inventory[0].qty, 3);
    }

    #[test]
    fn a_pickup_into_the_bank_leaves_the_inventory_alone() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"dropItem","items":{"1528":{"ItemID":1528,"sName":"Defeated Makai","iQty":1}}}"#));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"getDrop","bSuccess":1,"ItemID":1528,"CharItemID":77,"iQty":1,"iQtyNow":50,"bBank":1}"#)),
            [Event::ItemChanged { item: "Defeated Makai".into(), qty: 50, place: Place::Bank }]
        );
        assert!(world.inventory.is_empty());
    }

    #[test]
    fn players_come_move_and_go() {
        let mut world = joined();
        let entered = "<msg t='sys'><body action='uER' r='5'><u i='1003' m='0' s='0' p='2'><n><![CDATA[newbie]]></n></u></body></msg>";
        assert_eq!(feed(&mut world, entered), [Event::PlayerEntered { eid: EntityId(1003) }]);
        assert_eq!(feed(&mut world, entered), []);
        feed(&mut world, "%xt%uotls%-1%newbie%strFrame:r2,strPad:Left,px:500,py:375%");
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1003,"data":{"intLevel":42}}"#));
        let players = &world.area.as_ref().unwrap().players;
        let newbie = players.iter().find(|p| p.name == "newbie").map(|p| (p.cell.as_str(), p.level));
        assert_eq!(newbie, Some(("r2", 42)));
        feed(&mut world, "<msg t='sys'><body action='userGone' r='5'><user id='1003' /></body></msg>");
        feed(&mut world, "%xt%exitArea%-1%1002%ally%");
        assert!(world.area.as_ref().unwrap().players.is_empty());
    }

    #[test]
    fn kills_name_the_monster_and_pay_gold() {
        let mut world = joined();
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"addGoldExp","intGold":25,"intExp":10,"typ":"m","id":1}"#)),
            [Event::Killed { id: MonMapId(1), monster: "Mosquito".into() }]
        );
        assert_eq!(feed(&mut world, &json(r#"{"cmd":"addGoldExp","intGold":100,"intExp":100,"typ":"q"}"#)), []);
        assert_eq!(world.me.as_ref().unwrap().gold, 125);
    }

    #[test]
    fn a_login_elsewhere_and_notices_become_events() {
        let mut world = joined();
        assert_eq!(feed(&mut world, "%xt%multiLoginWarning%-1%"), [Event::LoggedInElsewhere]);
        assert_eq!(feed(&mut world, "%xt%notify%-1%Server restarting soon%"), [Event::ServerText { text: "Server restarting soon".into() }]);
    }

    #[test]
    fn auras_clear_expire_and_die_with_their_monster() {
        let mut world = joined();
        let now = Instant::now();
        reduce(
            &mut world,
            &Message::parse(&json(
                r#"{"cmd":"ct","a":[{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Haste","dur":5},{"nam":"Pact","persist":true},{"nam":"Forever","dur":1e300}]},
                                    {"cmd":"aura+","tInf":"m:1","auras":[{"nam":"Burn","dur":30}]}]}"#,
            ))
            .unwrap(),
            now,
        );
        let names = |w: &World| w.me.as_ref().unwrap().auras.iter().map(|a| a.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&world), ["Haste", "Pact", "Forever"]);

        reduce(&mut world, &Message::parse("%xt%hi%-1%").unwrap(), now + Duration::from_secs(6));
        assert_eq!(names(&world), ["Pact", "Forever"]);
        feed(&mut world, &json(r#"{"cmd":"clearAuras"}"#));
        assert_eq!(names(&world), ["Pact"]);

        feed(&mut world, &json(r#"{"cmd":"ct","m":{"1":{"intHP":0,"intState":0}}}"#));
        assert!(world.area.as_ref().unwrap().monsters[0].auras.is_empty());
    }

    #[test]
    fn counters_end_with_the_monster_or_the_aura() {
        let mut world = joined();
        let t0 = Instant::now();
        reduce(
            &mut world,
            &Message::parse(&json(
                r#"{"cmd":"ct","anims":[{"cInf":"m:1","msg":"Mosquito prepares a counter attack!!"}],
                "a":[{"cmd":"aura++","tInf":"m:1","auras":[{"nam":"Counter Attack","dur":6}]}]}"#,
            ))
            .unwrap(),
            t0,
        );
        let mosquito = |w: &World| w.area.as_ref().unwrap().monsters[0].countering(t0 + Duration::from_secs(1));
        assert!(mosquito(&world));
        let late = |w: &World| w.area.as_ref().unwrap().monsters[0].countering(t0 + Duration::from_secs(11));
        assert!(!late(&world));
        feed(&mut world, &json(r#"{"cmd":"ct","m":{"1":{"intHP":0,"intState":0}}}"#));
        assert!(!mosquito(&world));
        feed(&mut world, &json(r#"{"cmd":"mtls","id":1,"o":{"intHP":540,"intState":1}}"#));
        assert!(!mosquito(&world));
    }

    #[test]
    fn selling_pays_gold_and_balance_sets_it() {
        let mut world = joined();
        feed(
            &mut world,
            &json(r#"{"cmd":"loadInventoryBig","items":[{"ItemID":5,"CharItemID":77,"sName":"Sword","iQty":2,"iStk":5}],"factions":[]}"#),
        );
        feed(&mut world, &json(r#"{"cmd":"balance","intGold":1000}"#));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"sellItem","CharItemID":77,"iQty":1,"iQtyNow":1,"intAmount":250,"bCoins":false}"#)),
            [Event::ItemChanged { item: "Sword".into(), qty: 1, place: Place::Inventory }]
        );
        assert_eq!((world.me.as_ref().unwrap().gold, world.inventory[0].qty), (1250, 1));
        feed(&mut world, &json(r#"{"cmd":"sellItem","CharItemID":77,"iQty":1,"iQtyNow":0,"intAmount":5,"bCoins":true}"#));
        assert_eq!(world.me.as_ref().unwrap().gold, 1250);
        assert!(world.inventory.is_empty());
    }

    #[test]
    fn one_bad_item_does_not_lose_the_inventory() {
        let mut world = joined();
        let events = feed(
            &mut world,
            &json(r#"{"cmd":"loadInventoryBig","items":[{"ItemID":5,"sName":null,"iQty":1},{"ItemID":"oops"},{"ItemID":6,"sName":"Bow","iQty":1}],"factions":[]}"#),
        );
        assert_eq!(world.inventory.iter().map(|i| i.id).collect::<Vec<_>>(), [ItemId(5), ItemId(6)]);
        assert!(events.contains(&Event::InventoryLoaded { items: 2 }));
        assert!(events.iter().any(|e| matches!(e, Event::Unreadable { what } if what.starts_with("skipped 1 inventory item"))));
    }

    #[test]
    fn only_the_join_notice_counts_as_joining() {
        let mut world = joined();
        assert_eq!(
            feed(&mut world, "%xt%server%-1%You joined the party.%"),
            [Event::ServerText { text: "You joined the party.".into() }]
        );
        assert_eq!(
            feed(&mut world, "%xt%chatm%-1%hello%ally%"),
            [Event::Chat { channel: "zone".into(), sender: "ally".into(), text: "hello".into() }]
        );
    }

    #[test]
    fn chat_and_login_failures_become_events() {
        let mut world = World::default();
        assert_eq!(
            feed(&mut world, "%xt%chatm%-1%zone~hello there%ally%1002%-1%0%"),
            [Event::Chat { channel: "zone".into(), sender: "ally".into(), text: "hello there".into() }]
        );
        assert_eq!(
            feed(&mut world, "%xt%loginResponse%-1%0%-1%%Your game client is an older version.%"),
            [Event::LoginRejected { reason: "Your game client is an older version.".into() }]
        );
    }

    #[test]
    fn buying_from_a_loaded_shop_adds_the_item_and_spends_gold_or_ac() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"intLevel":24,"intGold":1000,"intCoins":50,"iBagSlots":30,"iBankSlots":40}}"#));
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadShop","shopinfo":{"ShopID":1348,"sName":"Featured Gear Shop","items":[
                {"ItemID":102658,"ShopItemID":68071,"sName":"Sword","iCost":300,"bCoins":false,"bUpg":false,"iStk":1,"iQty":1,"sType":"Sword","sES":"Weapon"},
                {"ItemID":5,"ShopItemID":6,"sName":"Hat","iCost":20,"bCoins":true,"bUpg":false,"iStk":1,"iQty":1,"sType":"Helm","sES":"he"}]}}"#,
            ),
        );
        assert_eq!(world.shops[&ShopId(1348)].items.len(), 2);
        assert_eq!(world.loaded_shop().map(|s| &s.info["items"][1]["sES"]), Some(&serde_json::json!("he")));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":102658,"CharItemID":77,"iQty":1,"bBank":0}"#)),
            [Event::ItemChanged { item: "Sword".into(), qty: 1, place: Place::Inventory }]
        );
        feed(&mut world, &json(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":5,"CharItemID":78,"iQty":1,"bBank":0}"#));
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.gold, me.coins, me.bag_slots, me.bank_slots), (700, 30, 30, 40));
        assert_eq!(world.inventory.iter().find(|i| i.name == "Sword").and_then(|i| i.char_item), Some(CharItemId(77)));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"buyItem","bitSuccess":0,"strMessage":"Not enough gold"}"#)),
            [Event::Refused { action: "buying".into(), reason: "Not enough gold".into() }]
        );
    }

    #[test]
    fn bank_moves_follow_the_replies() {
        let mut world = joined();
        let mut ignored = Vec::new();
        let stack = serde_json::json!({"ItemID":1528,"CharItemID":77,"sName":"Defeated Makai","iQty":49,"iStk":50,"bBank":true});
        world.set_bank(items_from(vec![stack], "bank item", &mut ignored));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"bankToInv","ItemID":"1528"}"#)),
            [Event::ItemChanged { item: "Defeated Makai".into(), qty: 49, place: Place::Inventory }]
        );
        assert!(world.bank.is_empty() && world.inventory[0].qty == 49);
        feed(&mut world, &json(r#"{"cmd":"bankFromInv","ItemID":1528,"bSuccess":1}"#));
        assert!(world.inventory.is_empty() && world.bank[0].qty == 49);
        feed(&mut world, &json(r#"{"cmd":"bankToInv","ItemID":"1528"}"#));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"bankFromInv","ItemID":1528,"bSuccess":0,"msg":"Your bank is full"}"#)),
            [Event::Refused { action: "depositing".into(), reason: "Your bank is full".into() }]
        );
        assert_eq!(world.inventory.len(), 1);
    }

    #[test]
    fn equipping_a_consumable_sets_the_item_skill_and_marks_it_equipped() {
        let mut world = joined();
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadInventoryBig","factions":[],"items":[
                {"ItemID":1,"CharItemID":11,"sName":"Health Potion","iQty":5,"sType":"Item","sLink":"potion","sMeta":"457"},
                {"ItemID":2,"CharItemID":12,"sName":"Mana Potion","iQty":5,"sType":"Item","sLink":"potion","sMeta":"458","bEquip":1}]}"#,
            ),
        );
        feed(&mut world, &json(r#"{"cmd":"seia","o":{"id":457,"cd":60000,"tgt":"f"},"iRes":1}"#));
        assert_eq!(world.me.as_ref().unwrap().item_skill, Some(457));
        assert_eq!(world.inventory.iter().map(|i| i.equipped).collect::<Vec<_>>(), [true, false]);
    }

    #[test]
    fn user_data_and_inventory_keep_the_fields_scripts_read() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"CharID":42,"intAQ":"1","intActivationFlag":"5"}}"#));
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.char_id, me.aq_linked, me.email_verified), (Some(42), true, true));

        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadInventoryBig","factions":[],"items":[{"ItemID":514,"CharItemID":7,"sName":"Scythe","iQty":1,"iStk":1,"sType":"Polearm",
                "sES":"Weapon","sFile":"items/polearms/polearm09.swf","sLink":"","sIcon":"iwpolearm","iLvl":"30"}]}"#,
            ),
        );
        let scythe = &world.inventory[0];
        assert_eq!((scythe.slot.as_str(), scythe.file.as_str(), scythe.icon.as_str(), scythe.level), ("Weapon", "items/polearms/polearm09.swf", "iwpolearm", 30));
    }

    #[test]
    fn going_away_and_coming_back_are_events() {
        let mut world = joined();
        assert_eq!(feed(&mut world, "%xt%uotls%-1%hero%afk:true%"), [Event::Afk { afk: true }]);
        assert_eq!(feed(&mut world, "%xt%uotls%-1%hero%afk:true%"), []);
        assert_eq!(feed(&mut world, "%xt%uotls%-1%hero%afk:false%"), [Event::Afk { afk: false }]);
    }

    #[test]
    fn house_items_live_apart_from_the_inventory_and_move_with_purchases_and_the_bank() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"iBagSlots":10,"iHouseSlots":2}}"#));
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadInventoryBig","factions":[],"items":[{"ItemID":1,"CharItemID":11,"sName":"Sword","iQty":1,"iStk":1,"sType":"Sword"}],
                "hitems":[{"ItemID":24,"CharItemID":21,"sName":"Crystal House","iQty":1,"iStk":1,"sType":"House","bHouse":true,"bEquip":true}]}"#,
            ),
        );
        assert_eq!((world.inventory.len(), world.house.len(), world.used(Pool::House)), (1, 1, 1));
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadShop","shopinfo":{"ShopID":42,"sName":"Furniture","items":[
                {"ItemID":70,"ShopItemID":71,"sName":"Blue Balloons","iCost":100,"bCoins":false,"iStk":1,"iQty":1,"sType":"Floor Item","sES":"hi","bHouse":true}]}}"#,
            ),
        );
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"buyItem","bitSuccess":1,"CharItemID":22,"iQty":1,"ItemID":70,"bBank":0}"#)),
            [Event::ItemChanged { item: "Blue Balloons".into(), qty: 1, place: Place::House }]
        );
        let balloons = world.house.iter().find(|i| i.id == ItemId(70)).unwrap().clone();
        assert!(world.full_for(&Item { id: ItemId(99), ..balloons.clone() }), "the house pool holds 2");
        assert!(!world.full_for(&Item { id: ItemId(98), kind: "Sword".into(), house: false, ..balloons }), "a bag item doesn't count house slots");

        feed(&mut world, &json(r#"{"cmd":"bankFromInv","ItemID":70,"bSuccess":1}"#));
        assert!(world.house.iter().all(|i| i.id != ItemId(70)) && world.bank.iter().any(|i| i.id == ItemId(70)));
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"bankToInv","ItemID":"70"}"#)),
            [Event::ItemChanged { item: "Blue Balloons".into(), qty: 1, place: Place::House }]
        );
        assert!(world.inventory.iter().all(|i| i.id != ItemId(70)));
    }

    #[test]
    fn an_enhancement_from_a_shop_updates_the_item_and_costs_gold() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"intGold":50000}}"#));
        feed(
            &mut world,
            &json(r#"{"cmd":"loadInventoryBig","factions":[],"items":[{"ItemID":540,"CharItemID":41,"sName":"Healer","iQty":302500,"iStk":1,"sType":"Class","sES":"ar","EnhID":1,"EnhLvl":10,"EnhPatternID":2}]}"#),
        );
        feed(
            &mut world,
            &json(r#"{"cmd":"enhanceItemShop","ItemIDs":[540],"EnhID":664,"EnhPID":7,"EnhLvl":100,"EnhRty":5,"EnhDPS":100,"EnhRng":10,"EnhName":"Healer Armor Enhancement","iCost":10060,"bSuccess":1}"#),
        );
        assert_eq!(world.inventory[0].enhancement, Some(Enhancement { id: 664, level: 100, pattern: 7 }));
        assert_eq!(world.me.as_ref().unwrap().gold, 50000 - 10060);
    }

    fn item(id: u32, name: &str, ac: bool) -> Value {
        serde_json::json!({"ItemID": id, "CharItemID": id + 1000, "sName": name, "iQty": 1, "iStk": 1, "bCoins": ac})
    }

    #[test]
    fn only_non_ac_items_fill_the_bank() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"iBankSlots":1}}"#));
        let mut ignored = Vec::new();
        world.set_bank(items_from(vec![item(1, "Crown", true), item(2, "Cape", true)], "bank item", &mut ignored));
        assert!(!world.bank_full());
        world.set_bank(items_from(vec![item(1, "Crown", true), item(3, "Sword", false)], "bank item", &mut ignored));
        assert!(world.bank_full());
    }

    #[test]
    fn a_pickup_into_the_bank_updates_the_bank_stack() {
        let mut world = joined();
        let mut ignored = Vec::new();
        let stack = serde_json::json!({"ItemID":1528,"CharItemID":77,"sName":"Defeated Makai","iQty":49,"iStk":50});
        world.set_bank(items_from(vec![stack], "bank item", &mut ignored));
        feed(&mut world, &json(r#"{"cmd":"dropItem","items":{"1528":{"ItemID":1528,"sName":"Defeated Makai","iQty":1}}}"#));
        feed(&mut world, &json(r#"{"cmd":"getDrop","bSuccess":1,"ItemID":1528,"CharItemID":77,"iQty":1,"iQtyNow":50,"bBank":1}"#));
        assert_eq!(world.bank[0].qty, 50);
        feed(&mut world, &json(r#"{"cmd":"addItems","items":{"1528":{"iQty":1,"iQtyNow":50,"CharItemID":77,"bBank":1}}}"#));
        assert_eq!((world.bank.len(), world.bank[0].qty), (1, 50));
    }

    #[test]
    fn a_purchase_is_priced_from_the_shop_it_was_bought_from() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"intGold":1000,"intCoins":50}}"#));
        let shop = |id: u32, cost: u32, ac: bool| {
            json(&format!(r#"{{"cmd":"loadShop","shopinfo":{{"ShopID":{id},"sName":"Shop","items":[{{"ItemID":7,"ShopItemID":{id},"sName":"Sword","iCost":{cost},"bCoins":{ac},"iStk":1,"iQty":1}}]}}}}"#))
        };
        feed(&mut world, &shop(1, 300, false));
        feed(&mut world, &shop(2, 20, true));
        world.buying = Some(ShopId(1));
        feed(&mut world, &json(r#"{"cmd":"buyItem","bitSuccess":1,"ItemID":7,"CharItemID":70,"iQty":1,"bBank":0}"#));
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.gold, me.coins), (700, 50));
    }

    #[test]
    fn ac_sales_add_ac() {
        let mut world = joined();
        feed(&mut world, &json(r#"{"cmd":"initUserData","uid":1001,"data":{"intGold":1000,"intCoins":50}}"#));
        feed(&mut world, &json(r#"{"cmd":"loadInventoryBig","items":[{"ItemID":5,"CharItemID":78,"sName":"Crown","iQty":1,"bCoins":true}],"factions":[]}"#));
        feed(&mut world, &json(r#"{"cmd":"sellItem","CharItemID":78,"iQty":1,"iQtyNow":0,"intAmount":400,"bCoins":true}"#));
        let me = world.me.as_ref().unwrap();
        assert_eq!((me.gold, me.coins), (1000, 450));
    }

    #[test]
    fn slot_five_empties_when_its_potions_run_out_and_a_refused_equip_does_not_fill_it() {
        let mut world = joined();
        feed(
            &mut world,
            &json(r#"{"cmd":"loadInventoryBig","items":[{"ItemID":11857,"CharItemID":80,"sName":"Health Potion","iQty":1,"sType":"Item","sLink":"potion","sMeta":"457"}],"factions":[]}"#),
        );
        feed(&mut world, &json(r#"{"cmd":"seia","o":{"id":457},"iRes":1}"#));
        feed(&mut world, &json(r#"{"cmd":"turnIn","sItems":"11857:1"}"#));
        assert_eq!(world.me.as_ref().unwrap().item_skill, None);
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"seia","o":{"id":457},"iRes":0}"#)),
            [Event::Refused { action: "equipping".into(), reason: "the server refused it".into() }]
        );
        assert_eq!(world.me.as_ref().unwrap().item_skill, None);
    }
}
