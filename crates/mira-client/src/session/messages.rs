use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use mira_protocol::{EntityId, JsonMessage, MonMapId, QuestId, XmlMessage, XtMessage, packet};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::bank;
use super::quests::{Answer, Verdict};
use crate::aura::{self, Target};
use crate::combat::{self, ITEM_SLOT};
use crate::enhancement::Enhanced;
use crate::event::{Event, NoticeKind};
use crate::faction::{self, AddedFaction, Caught, Faction, FactionData};
use crate::inventory::{
    AddedItems, LOADOUT_SLOTS, LoadInventory, Loadout, Picked, Removed, Sold, TurnIn, Worn,
};
use crate::player::{
    Balance, BoostUpdate, ClassChange, InitUserData, LevelUp, Me, Reward, SkillList, StatUpdate,
};
use crate::quest::{self, AchievementUpdate, QuestList, QuestUpdate};
use crate::room::{self, MonsterUpdate, MoveToArea, PlayerUpdate, Room, Tick, UserEntry, UserList};
use crate::shop::{Bought, LoadedShop};

use super::{
    BOOST_CHECK, Disconnect, JoinStage, Load, Op, Phase, Recovery, Session, ShopStage,
    required_version,
};

impl Session {
    pub(super) fn on_xml(&mut self, xml: &XmlMessage) {
        match xml.action.as_str() {
            "apiOK" if self.phase == Phase::Handshaking => {
                let login = packet::login(&self.username, &self.version, self.token.as_str());
                self.set_phase(Phase::LoggingIn);
                self.send(&login);
            }
            "logout" => {
                let why = self.closing.take().unwrap_or(Disconnect::Ended);
                self.close(why);
            }
            "uER" => {
                if let Some(user) = &xml.user {
                    self.on_player_entered(user);
                }
            }
            "userGone" => {
                if let Some(user) = &xml.user {
                    self.on_player_left(user.id);
                }
            }
            _ => {}
        }
    }

    pub(super) fn on_xt(&mut self, xt: &XtMessage) {
        let text = || xt.field(4).unwrap_or_default().to_owned();
        match xt.cmd() {
            "loginResponse" if self.phase == Phase::LoggingIn => self.on_login_response(xt),
            "server" => {
                let text = text();
                if text.to_ascii_lowercase().contains("invalid session") {
                    self.closing = Some(Disconnect::InvalidSession);
                }
                let joined = text.starts_with("You joined ");
                let goto_refused = text.to_ascii_lowercase().contains("goto")
                    && matches!(
                        self.current,
                        Some(Op::Join {
                            stage: JoinStage::Sent { goto: true, .. },
                            ..
                        })
                    );
                if goto_refused {
                    self.refusal = Some(text.clone());
                }
                self.emit(Event::Notice {
                    kind: NoticeKind::Server,
                    text,
                });
                if joined && let Some(room) = self.arriving.take() {
                    self.enter(room);
                }
            }
            "notify" => self.emit(Event::Notice {
                kind: NoticeKind::Notify,
                text: text(),
            }),
            "warning" => {
                let text = text();
                if matches!(
                    self.current,
                    Some(Op::Join {
                        stage: JoinStage::Sent { .. },
                        ..
                    })
                ) {
                    self.requested_spot = None;
                }
                self.combat.warned(&text, Instant::now());
                if combat::slow_down(&text) {
                    self.answer(Answer::SlowedDown);
                }
                self.refusal = Some(text.clone());
                self.emit(Event::Notice {
                    kind: NoticeKind::Warning,
                    text,
                });
            }
            "chatm" => {
                let raw = text();
                let (channel, text) = raw.split_once('~').unwrap_or(("", &raw));
                self.emit(Event::Chat {
                    channel: channel.to_owned(),
                    sender: xt.field(5).unwrap_or_default().to_owned(),
                    text: text.to_owned(),
                });
            }
            "whisper" => self.emit(Event::Whisper {
                sender: xt.field(5).unwrap_or_default().to_owned(),
                text: text(),
            }),
            "multiLoginWarning" => self.closing = Some(Disconnect::LoggedInElsewhere),
            "logoutWarning" => {
                let secs = xt.field(5).and_then(|secs| secs.parse().ok()).unwrap_or(0);
                self.closing = Some(Disconnect::Kicked { secs });
            }
            "zm" if xt.field(3) == Some("tryQuestComplete") => {
                if let Some(id) = xt.field(5).and_then(|id| id.parse().ok()) {
                    self.answer(Answer::TurnInEchoed(QuestId(id)));
                }
            }
            "exitArea" => {
                if let Some(eid) = xt.field(4).and_then(|eid| eid.parse().ok()) {
                    self.on_player_left(EntityId(eid));
                }
            }
            "uotls" => {
                if let (Some(name), Some(fields)) = (xt.field(4), xt.field(5)) {
                    self.on_player_moved(name, fields);
                }
            }
            "mtcid" => {
                if let Some(id) = xt.field(4).and_then(|id| id.parse().ok()) {
                    self.follow_door(id);
                }
            }
            "respawnMon" => {
                for id in text().split(',').filter_map(|id| id.parse().ok()) {
                    self.aggro_respawned(MonMapId(id));
                }
            }
            "resTimed" => {
                let to = match (xt.field(4), xt.field(5)) {
                    (Some(cell), Some(pad)) if !cell.is_empty() && !pad.is_empty() => {
                        Some((cell.to_owned(), pad.to_owned()))
                    }
                    _ => self.room.as_ref().map(|room| room.spawn_point.clone()),
                };
                if let Some((cell, pad)) = to {
                    let _ = self.move_to_cell(&cell, &pad);
                }
            }
            _ => {}
        }
    }

    fn follow_door(&mut self, id: i64) {
        let Some((cell, pad)) = self.room.as_ref().and_then(|room| {
            let cell = self.loaded_map()?.cell(&room.cell)?;
            let door = cell.doors.iter().find(|door| door.id == id)?;
            (!room.locked(&door.cell)).then(|| (door.cell.clone(), door.pad.clone()))
        }) else {
            return;
        };
        if self.move_to_cell_with(&cell, &pad, true).is_ok() {
            self.fetch_cellmates();
            self.aggro_cell();
        }
    }

    pub(super) fn on_login_response(&mut self, xt: &XtMessage) {
        let accepted = xt.field(4) == Some("true");
        match xt.field(5).and_then(|eid| eid.parse().ok()) {
            Some(eid) if accepted => {
                self.me = Some(EntityId(eid));
                self.character = Some(Me {
                    id: EntityId(eid),
                    name: self.username.clone(),
                    ..Me::default()
                });
                self.set_phase(Phase::Loading {
                    step: Load::Joining,
                });
                self.send(&packet::first_join());
                self.send(&packet::clear_ignore_list());
            }
            _ => {
                let message = xt.field(7).unwrap_or_default();
                let why = match required_version(message) {
                    Some(version) => Disconnect::ClientVersion(version),
                    None => Disconnect::Rejected(message.to_owned()),
                };
                self.close(why);
            }
        }
    }

    pub(super) fn on_json(&mut self, json: JsonMessage) -> Result<(), String> {
        fn read<T: DeserializeOwned>(json: &JsonMessage) -> Result<T, String> {
            T::deserialize(&json.data).map_err(|e| format!("{}: {e}", json.cmd))
        }
        if let Some(Value::Object(procs)) = json.data.get("procs") {
            let named = procs.iter().filter_map(|(id, proc)| {
                Some((id.parse().ok()?, proc.get("nam")?.as_str()?.to_owned()))
            });
            Arc::make_mut(&mut self.procs).extend(named);
        }
        match json.cmd.as_str() {
            "moveToArea" => {
                let area: MoveToArea = read(&json)?;
                let requested = self.requested_spot.take_if(|spot| area.is_for(&spot.map));
                let (room, mine) = Room::arriving(area, self.me, requested);
                if let Some(mine) = &mine {
                    self.patch_me(&mine.vitals);
                }
                if let (Some(mine), Some(character)) = (mine, &mut self.character) {
                    character.name = mine.name;
                    if character.away != mine.away {
                        character.away = mine.away;
                        self.emit(Event::AwayChanged { away: mine.away });
                    }
                }
                self.arriving = Some(room);
            }
            "initUserData" => {
                let entry: UserEntry = read(&json)?;
                if Some(entry.uid) != self.me {
                    self.learn_player(&entry);
                    return Ok(());
                }
                if self.phase
                    != (Phase::Loading {
                        step: Load::Character,
                    })
                {
                    return Ok(());
                }
                let user: InitUserData = read(&json)?;
                let (Some(me), Some(room)) = (self.me, &self.room) else {
                    return Ok(());
                };
                if user.uid == me {
                    let room = room.area;
                    if let Some(character) = &mut self.character {
                        character.load(&user.data, Instant::now());
                    }
                    self.set_phase(Phase::Loading {
                        step: Load::Inventory,
                    });
                    self.send(&packet::retrieve_inventory(room, me));
                }
            }
            "initUserDatas" => {
                for entry in read::<UserList>(&json)?.a {
                    self.learn_player(&entry);
                }
            }
            "balance" => {
                let balance: Balance = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.balance(balance);
                }
            }
            "xpboost" | "gboost" | "repboost" | "cpboost" => {
                let update: BoostUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.boosts.apply(&json.cmd, &update, Instant::now());
                }
            }
            "mtls" => {
                let update: MonsterUpdate = read(&json)?;
                self.patch_monster(update.id, &update.o);
            }
            "ct" | "cb" => {
                let tick: Tick = read(&json)?;
                for (id, patch) in &tick.monsters {
                    self.patch_monster(*id, patch);
                }
                for (name, patch) in &tick.players {
                    self.patch_player(name, patch);
                }
                for change in tick.auras {
                    self.apply_aura(change);
                }
                for result in &tick.sarsa {
                    self.settle(result);
                }
                for anim in &tick.anims {
                    let telegraph = anim.msg.as_deref().is_some_and(room::counter_warning);
                    if let (true, Some(Target::Monster(id))) =
                        (telegraph, anim.caster.as_deref().and_then(aura::target))
                        && let Some(monster) = self.room.as_mut().and_then(|r| r.monster(id))
                    {
                        let now = Instant::now();
                        let was = monster.countering(now);
                        monster.warn_counter(now);
                        if !was && monster.countering(now) {
                            self.emit(Event::CounterStarted { monster: id });
                        }
                    }
                }
            }
            "sars" => self.settle(&read(&json)?),
            "aura+" | "aura++" | "aura+p" | "aura=" | "aura-" | "aura--" => {
                self.apply_aura(read(&json)?);
            }
            "clearAuras" => {
                if let Some(character) = &mut self.character {
                    character.auras.clear_unless_persistent();
                }
            }
            "uotls" => {
                let update: PlayerUpdate = read(&json)?;
                self.patch_player(&update.name, &update.o.vitals);
                let room = self.room.as_mut();
                if let Some(jumped) = room
                    .and_then(|room| room.place_player(&update.name, update.o.cell, update.o.pad))
                {
                    self.emit(jumped);
                }
            }
            "event" => {
                let zone = json.data.pointer("/args/zoneSet").and_then(Value::as_str);
                if let (Some(zone), Some(room)) = (zone, &mut self.room) {
                    let zone = (!zone.is_empty()).then(|| zone.to_owned());
                    if room.zone != zone {
                        room.zone = zone.clone();
                        self.emit(Event::Zone { zone });
                    }
                }
            }
            "stu" => {
                let update: StatUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.apply_stats(update);
                }
            }
            "seia" => self.on_slot_filled(read(&json)?, Instant::now()),
            "sAct" => {
                let skills: SkillList = read(&json)?;
                if let Some(character) = &mut self.character {
                    let item = character.learn_skills(skills);
                    if std::mem::take(&mut self.class_changed) {
                        for skill in &character.skills {
                            self.combat.restart_clock(&skill.slot);
                        }
                    } else if item {
                        self.combat.restart_clock(ITEM_SLOT);
                    }
                }
            }
            "loadInventoryBig"
                if self.phase
                    == (Phase::Loading {
                        step: Load::Inventory,
                    }) =>
            {
                let loaded: LoadInventory = read(&json)?;
                let Some(character) = &self.character else {
                    return Ok(());
                };
                let (level, gold) = (character.level, character.gold);
                if let Some(character) = &mut self.character {
                    character.bank_count = loaded.bank_count;
                }
                let mut items = self.parse_each("inventory item", loaded.items);
                items.extend(self.parse_each("house item", loaded.hitems));
                self.inventory.items = Arc::new(items);
                self.factions = self
                    .parse_each::<FactionData>("faction", loaded.factions)
                    .into_iter()
                    .map(Faction::from)
                    .collect();
                self.ready_once = true;
                self.reconnects = 0;
                self.boost_check = Some(Instant::now() + BOOST_CHECK);
                if let Some(Recovery { why, .. }) = self.recovery.take() {
                    self.epoch.fetch_add(1, Ordering::SeqCst);
                    self.stale = Some(why);
                }
                self.set_phase(Phase::Ready);
                self.emit(Event::Ready {
                    level,
                    gold,
                    items: self.inventory.items.len(),
                });
            }
            "addItems" => {
                for (key, mut item) in read::<AddedItems>(&json)?.items {
                    let qty_now = item.qty_now.unwrap_or(0);
                    let event = if item.bank {
                        item.bank = false;
                        self.bank.as_mut().and_then(|bank| {
                            let mut inventory = crate::inventory::Inventory {
                                items: Arc::clone(bank),
                                ..Default::default()
                            };
                            let event = inventory.add(&key, item);
                            *bank = inventory.items;
                            event.map(|event| match event {
                                Event::ItemChanged { id, name, qty, .. } => Event::ItemChanged {
                                    id,
                                    name,
                                    qty,
                                    place: crate::Place::Bank,
                                },
                                event => event,
                            })
                        })
                    } else {
                        self.inventory.add(&key, item)
                    };
                    if let Some(event) = event {
                        if let Event::ItemChanged { id, place, .. } = event {
                            self.emit(Event::ItemReceived {
                                id,
                                qty: qty_now,
                                place,
                            });
                        }
                        self.emit(event);
                    }
                }
            }
            "dropItem" => {
                for (key, item) in read::<AddedItems>(&json)?.items {
                    if let Some(event) = self.inventory.offer(&key, item) {
                        self.emit(event);
                    }
                }
            }
            "getDrop" => {
                let mut picked: Picked = read(&json)?;
                let (id, ok) = (picked.id, picked.ok);
                if picked.bank && self.bank.is_some() {
                    let qty_now = picked.qty_now.unwrap_or(0);
                    picked.bank = false;
                    let mut inventory = crate::inventory::Inventory {
                        items: Arc::clone(self.bank.as_ref().unwrap()),
                        drops: self.inventory.drops.clone(),
                        ..Default::default()
                    };
                    let event = inventory.pick(picked);
                    self.bank = Some(inventory.items);
                    self.inventory.drops = inventory.drops;
                    if let Some(Event::ItemChanged { id, name, qty, .. }) = event {
                        self.emit(Event::ItemChanged {
                            id,
                            name,
                            qty,
                            place: crate::Place::Bank,
                        });
                        self.emit(Event::ItemReceived {
                            id,
                            qty: qty_now,
                            place: crate::Place::Bank,
                        });
                    }
                } else if let Some(event) = self.inventory.pick(picked) {
                    self.emit(event);
                }
                if ok {
                    self.emit(Event::ItemPickedUp { id });
                }
                self.picked(id, ok);
            }
            "turnIn" => {
                for event in self.inventory.turn_in(&read::<TurnIn>(&json)?) {
                    self.emit(event);
                }
            }
            "sellItem" => {
                let sold: Sold = read(&json)?;
                if let Some(event) = self.inventory.sell(&sold) {
                    self.emit(event);
                }
                if let Some(character) = &mut self.character {
                    character.receive(sold.amount, sold.coins);
                }
                self.emit(Event::ItemSold {
                    char_item_id: sold.char_item_id,
                    qty: sold.qty,
                    qty_now: sold.qty_now,
                    amount: sold.amount,
                    coins: sold.coins,
                });
                self.on_sold(sold.char_item_id);
            }
            "removeItem" => {
                if let Some(event) = self.inventory.remove(&read::<Removed>(&json)?) {
                    self.emit(event);
                }
            }
            "buyItem" => self.on_bought(read::<Bought>(&json)?),
            "enhanceItemShop" => {
                let enhanced: Enhanced = read(&json)?;
                for item in Arc::make_mut(&mut self.inventory.items)
                    .iter_mut()
                    .filter(|item| enhanced.items.contains(&item.id))
                {
                    item.enhancement = enhanced.fields.enhancement();
                }
                if let Some(character) = &mut self.character {
                    character.spend(enhanced.cost, false);
                }
            }
            "loadBank" => {
                let loaded: bank::Loaded = read(&json)?;
                if loaded.ok {
                    let items = self.parse_each::<crate::Item>("bank item", loaded.items);
                    let bank = self.bank.get_or_insert_with(|| Arc::new(Vec::new()));
                    let bank = Arc::make_mut(bank);
                    for item in items {
                        if let Some(held) = bank.iter_mut().find(|held| held.id == item.id) {
                            *held = item;
                        } else {
                            bank.push(item);
                        }
                    }
                    self.emit(Event::BankLoaded);
                }
            }
            "bankFromInv" => self.on_moved(read::<bank::Deposited>(&json)?.into()),
            "bankToInv" => self.on_moved(read::<bank::Withdrew>(&json)?.into()),
            "bankSwapInv" => self.on_moved(read::<bank::Swapped>(&json)?.into()),
            "equipItem" | "unequipItem" => {
                let worn: Worn = read(&json)?;
                if Some(worn.uid) == self.me {
                    let on = json.cmd == "equipItem";
                    if on && self.inventory.items.iter().any(|i| i.id == worn.id) {
                        self.clear_afk();
                    }
                    self.inventory.wear(worn.id, on);
                    self.on_worn(worn.id, on);
                }
            }
            "wearItem" | "unwearItem" => {
                let worn: Worn = read(&json)?;
                if Some(worn.uid) == self.me {
                    self.inventory.set_worn(worn.id, json.cmd == "wearItem");
                }
            }
            "wearLoadout" => {
                let loadout: Loadout = read(&json)?;
                if Some(loadout.uid) == self.me {
                    for slot in LOADOUT_SLOTS {
                        self.inventory.unwear_slot(slot);
                        if let Some(piece) = loadout.costume.get(slot) {
                            self.inventory.set_worn(piece.id, true);
                        }
                    }
                }
            }
            "addGoldExp" => {
                let reward: Reward = read(&json)?;
                if let Some(points) = self.character.as_mut().and_then(|c| c.gain(&reward)) {
                    self.inventory.set_class_points(points);
                }
                if let Some(id) = reward.faction {
                    faction::add_rep(&mut self.factions, id, reward.rep);
                }
                if let (Some(id), "m") = (reward.id, reward.source.as_str()) {
                    let name = self
                        .arriving
                        .as_ref()
                        .or(self.room.as_ref())
                        .and_then(|room| room.monsters.iter().find(|m| m.id == id))
                        .map(|m| m.name.clone())
                        .unwrap_or_default();
                    self.emit(Event::Killed {
                        id,
                        name,
                        gold: reward.gold,
                        exp: reward.exp,
                    });
                }
            }
            "CatchResult" => {
                let caught: Caught = read(&json)?;
                faction::add_rep(&mut self.factions, faction::FISHING, caught.result.rep);
            }
            "levelUp" => {
                let level: LevelUp = read(&json)?;
                if let Some(character) = &mut self.character {
                    let level = character.level_up(&level);
                    self.emit(Event::LeveledUp { level });
                }
            }
            "updateClass" => {
                let class: ClassChange = read(&json)?;
                if Some(class.uid) == self.me {
                    self.class_changed = true;
                    if let Some(character) = &mut self.character {
                        character.change_class(&class);
                    }
                } else if let Some(player) = self.room.as_mut().and_then(|r| r.player(class.uid)) {
                    player.class_name = Some(class.class_name);
                }
            }
            "addFaction" => {
                let added: AddedFaction = read(&json)?;
                faction::learn(&mut self.factions, added.faction.into());
            }
            "updateQuest" => {
                let update: QuestUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.quests.update(&update);
                }
            }
            "setAchievement" => {
                let update: AchievementUpdate = read(&json)?;
                if let Some(character) = &mut self.character {
                    character.quests.set_achievement(&update);
                }
            }
            "getQuests" => {
                let list: QuestList = read(&json)?;
                let ids = list.ids();
                for error in quest::learn(&mut self.quests, list) {
                    self.emit(Event::Unreadable { error });
                }
                self.answer(Answer::Quests(ids));
            }
            "loadShop" => {
                let shop = Arc::new(read::<LoadedShop>(&json)?.shopinfo);
                let id = shop.id;
                self.shop_maps.insert(id, shop.maps.clone());
                self.shop = Some(shop);
                if matches!(self.current, Some(Op::Shop { id: wanted, stage: ShopStage::Sent { .. }, .. }) if wanted == id)
                    && let Some(Op::Shop { reply, .. }) = self.current.take()
                {
                    self.replies.push((reply, Ok(())));
                }
            }
            "acceptQuest" => {
                let verdict: Verdict = read(&json)?;
                if verdict.ok && self.accepted_quests.insert(verdict.id) {
                    self.emit(Event::QuestAccepted { id: verdict.id });
                }
                self.answer(Answer::Accepted(verdict));
            }
            "ccqr" => {
                let verdict: Verdict = read(&json)?;
                if verdict.ok {
                    self.accepted_quests.remove(&verdict.id);
                    self.emit(Event::QuestCompleted { id: verdict.id });
                }
                self.answer(Answer::TurnedIn(verdict));
            }
            _ => {}
        }
        Ok(())
    }
}
