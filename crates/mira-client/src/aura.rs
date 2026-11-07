use std::time::{Duration, Instant};

use mira_protocol::{EntityId, MonMapId, flex};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Aura {
    pub name: String,
    pub icon: String,
    pub description: String,
    pub duration: f64,
    pub stacks: u32,
    pub value: Option<f64>,
    pub category: Option<String>,
    pub persist: bool,
    pub caster: Option<Caster>,
    pub effects: Vec<StatEffect>,
    #[serde(skip)]
    pub expires: Option<Instant>,
    #[serde(skip)]
    pub applied: Instant,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HudAura {
    pub name: String,
    pub icon: String,
    pub description: String,
    pub stacks: u32,
    pub persist: bool,
    pub duration: f64,
    #[serde(skip)]
    pub expires: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatEffect {
    #[serde(rename(deserialize = "sta"), default)]
    pub stat: String,
    #[serde(rename(deserialize = "typ"), default)]
    pub op: String,
    #[serde(rename(deserialize = "val"), default, deserialize_with = "flex::float")]
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Caster {
    Monster(MonMapId),
    Player(EntityId),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Auras {
    pub active: Vec<Aura>,
    pub passive: Vec<Aura>,
    pub hud: Vec<HudAura>,
}

pub(crate) enum Target {
    Monster(MonMapId),
    Player(EntityId),
}

impl Auras {
    pub(crate) fn apply(&mut self, change: AuraChange, now: Instant) -> Vec<AuraDelta> {
        let caster = change.caster.as_deref().and_then(caster);
        let mut deltas = Vec::new();
        let entries = match (change.auras, change.aura) {
            (Some(auras), _) => auras,
            (None, Some(aura)) => vec![aura],
            (None, None) => Vec::new(),
        };
        for data in &entries {
            self.sync_hud(&change.cmd, data, now);
        }
        match change.cmd.as_str() {
            "aura+" | "aura++" => {
                for data in entries {
                    let is_new = data.is_new;
                    let aura = data.into_aura(caster, now);
                    if is_new {
                        deltas.push(AuraDelta::Added {
                            name: aura.name.clone(),
                            stacks: aura.stacks,
                        });
                        self.active.push(aura);
                        continue;
                    }
                    for held in self
                        .active
                        .iter_mut()
                        .filter(|held| held.name == aura.name && held.caster == aura.caster)
                    {
                        held.duration = aura.duration;
                        held.value = aura.value;
                        held.expires = aura.expires;
                    }
                }
            }
            "aura+p" => {
                self.passive
                    .extend(entries.into_iter().map(|data| data.into_aura(caster, now)));
            }
            "aura-" | "aura--" => {
                for AuraData { name, .. } in entries {
                    remove_first(&mut self.passive, &name);
                    if remove_first(&mut self.active, &name) {
                        deltas.push(AuraDelta::Removed { name });
                    }
                }
            }
            _ => {}
        }
        deltas
    }

    fn sync_hud(&mut self, cmd: &str, data: &AuraData, now: Instant) {
        if data.name.is_empty() {
            return;
        }
        let at = self.hud.iter().position(|hud| hud.name == data.name);
        match (cmd, at) {
            ("aura=", Some(at)) => {
                if let Some(stacks) = data.stacks {
                    self.hud[at].stacks = stacks.max(1);
                }
            }
            ("aura-" | "aura--", Some(at)) => match data.stacks.filter(|stacks| *stacks > 0) {
                Some(stacks) => {
                    let hud = &mut self.hud[at];
                    hud.stacks = stacks;
                    if let (Some(duration), Some(_)) = (data.duration, hud.expires) {
                        hud.expires = expiry(duration, now);
                        hud.duration = hud.duration.max(duration);
                    }
                }
                None => {
                    self.hud.remove(at);
                }
            },
            ("aura+" | "aura++", at) => {
                let at = at.unwrap_or_else(|| {
                    self.hud.push(HudAura {
                        name: data.name.clone(),
                        icon: String::new(),
                        description: String::new(),
                        stacks: 1,
                        persist: false,
                        duration: 0.0,
                        expires: None,
                    });
                    self.hud.len() - 1
                });
                let hud = &mut self.hud[at];
                hud.stacks = data.stacks.unwrap_or(1);
                if !data.icon.is_empty() {
                    hud.icon = data.icon.clone();
                }
                if !data.description.is_empty() {
                    hud.description = data.description.clone();
                }
                hud.persist = data.persist;
                let duration = data.duration.unwrap_or(0.0);
                let timed = data.timing.as_ref().and_then(Value::as_str) == Some("s");
                if timed && duration > 0.0 {
                    if data.is_new || hud.duration <= duration {
                        hud.duration = duration;
                    }
                    hud.expires = expiry(duration, now);
                } else {
                    hud.duration = 0.0;
                    hud.expires = None;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn clear_passives(&mut self) {
        self.passive.clear();
    }

    pub(crate) fn clear(&mut self) {
        self.active.clear();
        self.hud.clear();
    }

    pub(crate) fn clear_unless_persistent(&mut self) {
        self.active.retain(|aura| aura.persist);
        self.hud.retain(|hud| hud.persist);
    }
}

fn expiry(secs: f64, now: Instant) -> Option<Instant> {
    Some(secs)
        .filter(|secs| *secs > 0.0)
        .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
        .and_then(|lasts| now.checked_add(lasts))
}

fn remove_first(auras: &mut Vec<Aura>, name: &str) -> bool {
    let at = auras.iter().position(|aura| aura.name == name);
    at.map(|at| auras.remove(at)).is_some()
}

pub(crate) enum AuraDelta {
    Added { name: String, stacks: u32 },
    Removed { name: String },
}

fn caster(info: &str) -> Option<Caster> {
    match target(info)? {
        Target::Monster(id) => Some(Caster::Monster(id)),
        Target::Player(eid) => Some(Caster::Player(eid)),
    }
}

pub(crate) fn target(info: &str) -> Option<Target> {
    let (kind, id) = info.split_once(':')?;
    let id = id.parse().ok()?;
    match kind {
        "m" => Some(Target::Monster(MonMapId(id))),
        "p" => Some(Target::Player(EntityId(id))),
        _ => None,
    }
}

#[derive(Deserialize)]
pub(crate) struct AuraChange {
    #[serde(default)]
    cmd: String,
    #[serde(rename = "cInf", default)]
    caster: Option<String>,
    #[serde(rename = "tInf", default)]
    pub(crate) target: Option<String>,
    #[serde(default)]
    auras: Option<Vec<AuraData>>,
    #[serde(default)]
    aura: Option<AuraData>,
}

#[derive(Deserialize)]
struct AuraData {
    #[serde(rename = "nam", default)]
    name: String,
    #[serde(default)]
    icon: String,
    #[serde(rename = "desc", default)]
    description: String,
    #[serde(rename = "dur", default, deserialize_with = "flex::opt_float")]
    duration: Option<f64>,
    #[serde(rename = "t", default)]
    timing: Option<Value>,
    #[serde(rename = "stk", default, deserialize_with = "flex::opt_int")]
    stacks: Option<u32>,
    #[serde(rename = "val", default)]
    value: Option<Value>,
    #[serde(rename = "cat", default)]
    category: Option<String>,
    #[serde(rename = "isNew", default, deserialize_with = "flex::bool")]
    is_new: bool,
    #[serde(default, deserialize_with = "flex::bool")]
    persist: bool,
    #[serde(rename = "e", default)]
    effects: Vec<StatEffect>,
}

impl AuraChange {
    pub(crate) fn removes(&self) -> bool {
        self.cmd.starts_with("aura-")
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        let added = self.auras.iter().flatten().map(|a| a.name.as_str());
        added.chain(self.aura.iter().map(|a| a.name.as_str()))
    }
}

impl AuraData {
    fn into_aura(self, caster: Option<Caster>, now: Instant) -> Aura {
        let duration = self.duration.unwrap_or(0.0);
        Aura {
            expires: expiry(duration, now),
            applied: now,
            name: self.name,
            icon: self.icon,
            description: self.description,
            duration,
            stacks: self.stacks.unwrap_or(1),
            value: self.value.and_then(|v| flex::float(v).ok()),
            category: self.category,
            persist: self.persist,
            caster,
            effects: self.effects,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_moves_the_expiry() {
        let change =
            |json: serde_json::Value| -> AuraChange { serde_json::from_value(json).unwrap() };
        let t0 = Instant::now();
        let mut auras = Auras::default();
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "m:1",
                "auras": [{"nam": "Burn", "dur": 6, "icon": "burn", "desc": "Burn description", "isNew": true}]})),
            t0,
        );
        assert_eq!(auras.active[0].expires, Some(t0 + Duration::from_secs(6)));
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "m:1",
                "auras": [{"nam": "Burn", "dur": 10}]})),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(auras.active[0].expires, Some(t0 + Duration::from_secs(11)));
        assert_eq!(auras.active[0].applied, t0);
        assert_eq!(auras.active[0].icon, "burn");
        assert_eq!(auras.active[0].description, "Burn description");
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "m:1",
                "auras": [{"nam": "Burn", "dur": 1e300}]})),
            t0,
        );
        assert_eq!(auras.active[0].expires, None);
    }

    #[test]
    fn the_hud_keeps_one_entry_per_name_with_the_latest_stacks() {
        let change =
            |json: serde_json::Value| -> AuraChange { serde_json::from_value(json).unwrap() };
        let stacks = |auras: &Auras| -> Vec<(String, u32)> {
            auras
                .hud
                .iter()
                .map(|hud| (hud.name.clone(), hud.stacks))
                .collect()
        };
        let t0 = Instant::now();
        let mut auras = Auras::default();
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "p:1",
                "auras": [{"nam": "Feeding", "dur": 10, "t": "s", "stk": 2, "isNew": true},
                          {"nam": "Doom", "dur": 0, "isNew": true}]})),
            t0,
        );
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "p:2",
                "auras": [{"nam": "Feeding", "dur": 10, "t": "s", "stk": 5, "isNew": true}]})),
            t0,
        );
        assert_eq!(auras.active.len(), 3);
        assert_eq!(stacks(&auras), [("Feeding".into(), 5), ("Doom".into(), 1)]);
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "p:1",
                "auras": [{"nam": "Feeding", "dur": 10, "t": "s", "stk": 7}]})),
            t0,
        );
        assert_eq!(stacks(&auras)[0], ("Feeding".into(), 7));
        auras.apply(
            change(serde_json::json!({"cmd": "aura=", "auras": [{"nam": "Doom", "stk": 9}]})),
            t0,
        );
        assert_eq!(stacks(&auras)[1], ("Doom".into(), 9));
        auras.apply(
            change(
                serde_json::json!({"cmd": "aura-", "aura": {"nam": "Feeding", "stk": 3, "dur": 4}}),
            ),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(stacks(&auras), [("Feeding".into(), 3), ("Doom".into(), 9)]);
        assert_eq!(auras.hud[0].expires, Some(t0 + Duration::from_secs(5)));
        assert_eq!(auras.hud[0].duration, 10.0);
        auras.apply(
            change(serde_json::json!({"cmd": "aura-", "aura": {"nam": "Feeding"}})),
            t0,
        );
        assert_eq!(stacks(&auras), [("Doom".into(), 9)]);
    }
}
