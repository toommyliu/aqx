use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use aqx_protocol::de::{flag, int, text};
use aqx_protocol::{AreaId, CharItemId, EntityId, FactionId, ItemId, Json, Message, MonId, MonMapId, Xml, Xt};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::event::{Event, Place};

const RANK_REP: [u32; 9] = [900, 3600, 10000, 22500, 44100, 78400, 129600, 202500, 302500];
const GOLD_CAP: i64 = 100_000_000;
const COUNTER_AURA: &str = "Counter Attack";
const COUNTER_WARNING: &str = "prepares a counter attack";
const COUNTER_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct World {
    pub me: Option<Me>,
    pub area: Option<Area>,
    pub inventory: Vec<Item>,
    pub temp: Vec<Item>,
    pub factions: Vec<Faction>,
    pub drops: Vec<Item>,
    pub rev: u64,
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
    pub vitals: Vitals,
    pub auras: Vec<Aura>,
    pub skills: Vec<Skill>,
    pub stats: BTreeMap<String, f64>,
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
    pub monsters: Vec<Monster>,
    pub players: Vec<Player>,
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
}

#[derive(Clone)]
pub struct Faction {
    pub id: FactionId,
    pub name: String,
    pub rep: u32,
}

impl Faction {
    pub fn rank(&self) -> u32 {
        1 + RANK_REP.iter().filter(|&&threshold| self.rep >= threshold).count() as u32
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
        Message::Xml(xml) => world.on_xml(xml),
        Message::Xt(xt) => world.on_xt(xt, &mut events),
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
    #[serde(rename = "bTemp", default, deserialize_with = "flag")]
    temp: bool,
    #[serde(rename = "bBank", default, deserialize_with = "flag")]
    bank: bool,
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

fn qty(n: i64) -> u32 {
    n.clamp(0, u32::MAX as i64) as u32
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
    fn on_xml(&mut self, xml: &Xml) {
        let me = self.me.as_ref().map(|m| m.eid);
        match xml.action {
            Some("joinOK") => {
                let Some(id) = xml.room() else { return };
                let players = xml.users().filter(|(eid, _)| Some(*eid) != me).map(|(eid, name)| Player::new(eid, name)).collect();
                self.area = Some(Area { id, name: String::new(), map: String::new(), monsters: Vec::new(), players });
            }
            Some("uER") => {
                if let Some(area) = &mut self.area {
                    for (eid, name) in xml.users() {
                        if Some(eid) != me && !area.players.iter().any(|p| p.eid == eid) {
                            area.players.push(Player::new(eid, name));
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

    fn on_xt(&mut self, xt: &Xt, events: &mut Vec<Event>) {
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
                match &self.area {
                    Some(area) if text.starts_with("You joined \"") => {
                        events.push(Event::Joined { area: area.name.clone(), players: area.players.len() + 1 })
                    }
                    _ => events.push(Event::ServerText { text: text.into() }),
                }
            }
            "warning" => events.push(Event::Warning { text: xt.arg(0).unwrap_or_default().into() }),
            "notify" | "logoutWarning" => events.push(Event::ServerText { text: xt.arg(0).unwrap_or_default().into() }),
            "multiLoginWarning" => events.push(Event::LoggedInElsewhere),
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
                for (key, value) in fields.split(',').filter_map(|kv| kv.split_once(':')) {
                    match key {
                        "strFrame" => patch.cell = Some(value.into()),
                        "strPad" => patch.pad = Some(value.into()),
                        _ => {}
                    }
                }
                self.patch_named(name, &patch, events);
            }
            _ => {}
        }
    }

    fn on_json(&mut self, json: &Json, now: Instant, events: &mut Vec<Event>) -> serde_json::Result<()> {
        match json.cmd {
            "moveToArea" => self.move_to_area(json.decode()?, events),
            "initUserData" => {
                let init: Init = json.decode()?;
                self.user_data(init.uid, init.data);
            }
            "initUserDatas" => {
                #[derive(Deserialize)]
                struct Inits {
                    a: Vec<Init>,
                }
                for init in json.decode::<Inits>()?.a {
                    self.user_data(init.uid, init.data);
                }
            }
            "uotls" => {
                #[derive(Deserialize)]
                struct Update {
                    unm: String,
                    o: Patch,
                }
                let update: Update = json.decode()?;
                self.patch_named(&update.unm, &update.o, events);
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
                    self.patch_named(name, patch, events);
                }
                for (id, patch) in &tick.m {
                    if let Some(m) = id.parse().ok().and_then(|id| self.monster(MonMapId(id))) {
                        m.apply(patch);
                    }
                }
                for change in tick.a {
                    self.aura(change, now);
                }
                for anim in tick.anims.iter().filter(|a| a.msg.contains(COUNTER_WARNING)) {
                    if let Some(m) = self.monster_ref(&anim.caster) {
                        m.counter_warned = Some(now);
                    }
                }
            }
            "aura+" | "aura++" | "aura-" | "aura--" => self.aura(json.decode()?, now),
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
                    factions: Vec<Value>,
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
                self.inventory = each::<ItemData>(inventory.items, "inventory item", events)
                    .into_iter()
                    .filter_map(|d| d.id.map(|id| d.into_item(id)))
                    .collect();
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
                        let qty = qty(data.qty_now.unwrap_or(data.qty));
                        events.push(Event::ItemChanged { item: data.name, qty, place: Place::Bank });
                        continue;
                    }
                    let temp = data.temp || data.char_item.is_none();
                    let (list, place) = if temp { (&mut self.temp, Place::Temp) } else { (&mut self.inventory, Place::Inventory) };
                    let gained = data.qty;
                    let item = match list.iter_mut().find(|i| i.id == id) {
                        Some(item) => {
                            item.qty = qty(data.qty_now.unwrap_or(item.qty as i64 + gained));
                            item.clone()
                        }
                        None => {
                            let item = data.into_item(id);
                            list.push(item.clone());
                            item
                        }
                    };
                    events.push(Event::ItemChanged { item: item.name, qty: item.qty, place });
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
                    events.push(Event::DropOffered { item: item.name.clone(), qty: item.qty });
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
                let place = if picked.bank { Place::Bank } else { Place::Inventory };
                events.push(Event::ItemChanged { item: item.name.clone(), qty: item.qty, place });
                if !picked.bank {
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
                for (id, n) in json.decode::<TurnIn>()?.items.split(',').filter_map(|pair| pair.split_once(':')) {
                    let (Ok(id), Ok(n)) = (id.parse().map(ItemId), n.parse::<u32>()) else { continue };
                    for (list, place) in [(&mut self.inventory, Place::Inventory), (&mut self.temp, Place::Temp)] {
                        if let Some(at) = list.iter().position(|i| i.id == id) {
                            list[at].qty = list[at].qty.saturating_sub(n);
                            events.push(Event::ItemChanged { item: list[at].name.clone(), qty: list[at].qty, place });
                            if list[at].qty == 0 {
                                list.remove(at);
                            }
                            break;
                        }
                    }
                }
            }
            "addGoldExp" => {
                #[derive(Deserialize)]
                struct Reward {
                    #[serde(rename = "intGold", default, deserialize_with = "int")]
                    gold: i64,
                    #[serde(default)]
                    typ: String,
                    id: Option<MonMapId>,
                }
                let reward: Reward = json.decode()?;
                if let Some(me) = &mut self.me {
                    me.gold += reward.gold;
                }
                if let ("m", Some(id)) = (reward.typ.as_str(), reward.id) {
                    let monster = self.monster(id).map_or_else(|| format!("monster {id}"), |m| m.name.clone());
                    events.push(Event::Killed { monster });
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
                if let Some(me) = self.me.as_mut().filter(|me| !sold.ac && me.gold < GOLD_CAP) {
                    me.gold += sold.amount;
                }
                if let Some(at) = self.inventory.iter().position(|i| i.char_item == Some(sold.char_item)) {
                    let item = &mut self.inventory[at];
                    item.qty = item.qty.saturating_sub(qty(sold.qty));
                    events.push(Event::ItemChanged { item: item.name.clone(), qty: item.qty, place: Place::Inventory });
                    if item.qty == 0 {
                        self.inventory.remove(at);
                    }
                }
            }
            "balance" => {
                #[derive(Deserialize)]
                struct Balance {
                    #[serde(rename = "intGold", deserialize_with = "int")]
                    gold: i64,
                }
                let balance: Balance = json.decode()?;
                if let Some(me) = &mut self.me {
                    me.gold = balance.gold;
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
                            target: match s.tgt.as_str() {
                                "f" => SkillTarget::Friendly,
                                "s" => SkillTarget::Myself,
                                _ => SkillTarget::Hostile,
                            },
                            max_targets: qty(s.max_targets).max(1),
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
                }
                let class: Class = json.decode()?;
                if let Some(me) = self.me.as_mut().filter(|m| m.eid == class.uid) {
                    me.class = class.name;
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
            _ => {}
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
        self.area = Some(Area { id: m.id, name: m.name, map: m.map, monsters, players });
        for avatar in each::<Avatar>(m.avatars, "player", events) {
            let me = self.me.as_mut().filter(|me| me.eid == avatar.eid);
            if let Some(me) = me {
                me.apply(&avatar.patch);
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

    fn user_data(&mut self, uid: EntityId, data: UserData) {
        if let Some(me) = self.me.as_mut().filter(|m| m.eid == uid) {
            me.char_id = data.char_id.or(me.char_id);
            me.level = qty(data.level);
            me.gold = data.gold.unwrap_or(me.gold);
            if let Some(class) = data.class {
                me.class = class;
            }
        } else if let Some(player) = self.area.as_mut().and_then(|a| a.players.iter_mut().find(|p| p.eid == uid)) {
            player.level = qty(data.level);
        }
    }

    fn patch_named(&mut self, name: &str, patch: &Patch, events: &mut Vec<Event>) {
        if let Some(me) = self.me.as_mut().filter(|m| m.user.eq_ignore_ascii_case(name)) {
            let was = me.vitals.state;
            me.apply(patch);
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

    fn aura(&mut self, change: AuraChange, now: Instant) {
        let Some(auras) = self.auras_of(&change.target) else { return };
        match change.cmd.as_str() {
            "aura+" | "aura++" => {
                for add in change.auras {
                    let until = add
                        .dur
                        .filter(|d| *d > 0.0)
                        .and_then(|d| Duration::try_from_secs_f64(d).ok())
                        .and_then(|d| now.checked_add(d));
                    let aura = Aura { name: add.nam, stacks: add.stk.unwrap_or(1), until, persist: add.persist };
                    match auras.iter_mut().find(|a| a.name == aura.name) {
                        Some(existing) => *existing = aura,
                        None => auras.push(aura),
                    }
                }
            }
            "aura-" | "aura--" => {
                let gone: Vec<String> = change.auras.into_iter().map(|a| a.nam).chain(change.aura.map(|a| a.nam)).collect();
                auras.retain(|a| !gone.contains(&a.name));
                if gone.iter().any(|name| name == COUNTER_AURA)
                    && let Some(m) = self.monster_ref(&change.target)
                {
                    m.counter_warned = None;
                }
            }
            _ => {}
        }
    }
}

impl Me {
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
            vitals: Vitals::default(),
            auras: Vec::new(),
            skills: Vec::new(),
            stats: BTreeMap::new(),
        }
    }

    fn apply(&mut self, patch: &Patch) {
        patch.place(&mut self.vitals, &mut self.cell, &mut self.pad);
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
}

#[derive(Deserialize)]
struct MoveToArea {
    #[serde(rename = "areaId")]
    id: AreaId,
    #[serde(rename = "areaName")]
    name: String,
    #[serde(rename = "strMapName")]
    map: String,
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
    fn items_follow_drops_pickups_and_turn_ins() {
        let mut world = joined();
        feed(
            &mut world,
            &json(
                r#"{"cmd":"loadInventoryBig","items":[{"ItemID":1528,"CharItemID":77,"sName":"Defeated Makai","iQty":3,"iStk":50,"sType":"Quest Item","sES":"None"}],
                "factions":[{"CharFactionID":"9","FactionID":"46","sName":"Aegis","iRep":"44100"}]}"#,
            ),
        );
        assert_eq!(world.factions[0].rank(), 6);
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
            [Event::DropOffered { item: "Defeated Makai".into(), qty: 1 }]
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
        feed(&mut world, "<msg t='sys'><body action='uER' r='5'><u i='1003' m='0' s='0' p='2'><n><![CDATA[newbie]]></n></u></body></msg>");
        feed(&mut world, "%xt%uotls%-1%newbie%strFrame:r2,strPad:Left,px:500,py:375%");
        let players = &world.area.as_ref().unwrap().players;
        assert_eq!(players.iter().find(|p| p.name == "newbie").map(|p| p.cell.as_str()), Some("r2"));
        feed(&mut world, "<msg t='sys'><body action='userGone' r='5'><user id='1003' /></body></msg>");
        feed(&mut world, "%xt%exitArea%-1%1002%ally%");
        assert!(world.area.as_ref().unwrap().players.is_empty());
    }

    #[test]
    fn kills_name_the_monster_and_pay_gold() {
        let mut world = joined();
        assert_eq!(
            feed(&mut world, &json(r#"{"cmd":"addGoldExp","intGold":25,"intExp":10,"typ":"m","id":1}"#)),
            [Event::Killed { monster: "Mosquito".into() }]
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
}
