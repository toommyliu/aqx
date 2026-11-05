use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use mira_protocol::{EntityId, JsonMessage, MonMapId, XmlMessage, XtMessage, packet};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::bank;
use super::quests::{Answer, Verdict};
use crate::aura::{self, Target};
use crate::combat::{self, ITEM_SLOT};
use crate::event::{Event, NoticeKind};
use crate::faction::{self, AddedFaction, Faction, FactionData};
use crate::inventory::{AddedItems, LoadInventory, Picked, Sold, TurnIn, Worn};
use crate::player::{
    BoostUpdate, ClassChange, InitUserData, LevelUp, Me, Reward, SkillList, StatUpdate,
};
use crate::quest::{self, AchievementUpdate, QuestList, QuestUpdate};
use crate::room::{
    self, MonsterUpdate, MoveToArea, PlayerUpdate, Room, SPAWN, Tick, UserEntry, UserList,
};
use crate::shop::{Bought, LoadedShop};

use super::{
    BOOST_CHECK, Disconnect, JoinStage, Load, Op, Phase, Respawn, Session, ShopStage,
    required_version,
};

impl Session {
    pub(super) fn on_xml(&mut self, xml: &XmlMessage) {
        match xml.action.as_str() {
            "apiOK" if self.phase == Phase::Handshaking => {
                let login = packet::login(&self.username, &self.version, self.token.as_str());
                self.send(&login);
                self.set_phase(Phase::LoggingIn);
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
            "zm" if xt.field(3) == Some("tryQuestComplete") => self.answer(Answer::TurnInEchoed),
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
            "respawnMon" => {
                for id in text().split(',').filter_map(|id| id.parse().ok()) {
                    self.aggro_respawned(MonMapId(id));
                }
            }
            "resTimed" => {
                let (cell, pad) = match (xt.field(4), xt.field(5)) {
                    (Some(cell), Some(pad)) if !cell.is_empty() && !pad.is_empty() => {
                        (cell.to_owned(), pad.to_owned())
                    }
                    _ => (SPAWN.0.to_owned(), SPAWN.1.to_owned()),
                };
                let _ = self.move_to_cell(&cell, &pad);
                let died_in = self.died_in.take();
                let room = self.room.as_ref();
                let back = match &self.respawn {
                    Respawn::Spawn => None,
                    Respawn::WhereDied => died_in
                        .filter(|(area, ..)| room.is_some_and(|room| room.area == *area))
                        .map(|(_, cell, pad)| (cell, pad)),
                    Respawn::At { cell, pad } => Some((cell.clone(), pad.clone())),
                };
                if let Some((cell, pad)) =
                    back.filter(|(cell, _)| room.is_some_and(|room| room.cell != *cell))
                {
                    let _ = self.move_to_cell(&cell, &pad);
                }
            }
            _ => {}
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
                self.send(&packet::first_join());
                self.send(&packet::clear_ignore_list());
                self.set_phase(Phase::Loading {
                    step: Load::Joining,
                });
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
                if let (Some(mine), Some(character)) = (mine, &mut self.character) {
                    character.apply(&mine.vitals);
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
                    self.send(&packet::retrieve_inventory(room, me));
                    self.set_phase(Phase::Loading {
                        step: Load::Inventory,
                    });
                }
            }
            "initUserDatas" => {
                for entry in read::<UserList>(&json)?.a {
                    self.learn_player(&entry);
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
            "aura+" | "aura++" | "aura+p" | "aura-" | "aura--" => {
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
                if let Some(room) = &mut self.room {
                    room.place_player(&update.name, update.o.cell, update.o.pad);
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
                if let Some(character) = &mut self.character
                    && character.learn_skills(skills)
                {
                    self.combat.restart_clock(ITEM_SLOT);
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
                if let Some(why) = self.lost.take() {
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
                for (key, item) in read::<AddedItems>(&json)?.items {
                    if let Some(event) = self.inventory.add(&key, item) {
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
                let picked: Picked = read(&json)?;
                let (id, ok) = (picked.id, picked.ok);
                if let Some(event) = self.inventory.pick(picked) {
                    self.emit(event);
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
                self.on_sold(sold.char_item_id);
            }
            "buyItem" => self.on_bought(read::<Bought>(&json)?),
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
                for error in quest::learn(&mut self.quests, read::<QuestList>(&json)?) {
                    self.emit(Event::Unreadable { error });
                }
                self.answer(Answer::Quests);
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
