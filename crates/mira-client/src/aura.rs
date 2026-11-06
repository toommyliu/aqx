use std::time::{Duration, Instant};

use mira_protocol::{EntityId, MonMapId, flex};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Aura {
    pub name: String,
    pub duration: f64,
    pub stacks: u32,
    pub value: Option<f64>,
    pub category: Option<String>,
    pub persist: bool,
    pub caster: Option<Caster>,
    pub effects: Vec<StatEffect>,
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
}

pub(crate) enum Target {
    Monster(MonMapId),
    Player(EntityId),
}

impl Auras {
    pub(crate) fn apply(&mut self, change: AuraChange, now: Instant) -> Vec<AuraDelta> {
        let caster = change.caster.as_deref().and_then(caster);
        let mut deltas = Vec::new();
        match change.cmd.as_str() {
            "aura+" | "aura++" => {
                for data in change.auras.into_iter().flatten() {
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
                self.passive.extend(
                    change
                        .auras
                        .into_iter()
                        .flatten()
                        .map(|data| data.into_aura(caster, now)),
                );
            }
            "aura-" | "aura--" => {
                let names: Vec<String> = match (change.auras, change.aura) {
                    (Some(auras), _) => auras.into_iter().map(|a| a.name).collect(),
                    (None, Some(removed)) => vec![removed.name],
                    (None, None) => Vec::new(),
                };
                for name in names {
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

    pub(crate) fn clear_passives(&mut self) {
        self.passive.clear();
    }

    pub(crate) fn clear(&mut self) {
        self.active.clear();
    }

    pub(crate) fn clear_unless_persistent(&mut self) {
        self.active.retain(|aura| aura.persist);
    }
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
    aura: Option<AuraName>,
}

#[derive(Deserialize)]
struct AuraData {
    #[serde(rename = "nam", default)]
    name: String,
    #[serde(rename = "dur", default, deserialize_with = "flex::float")]
    duration: f64,
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
        let expires = Some(self.duration)
            .filter(|secs| *secs > 0.0)
            .and_then(|secs| Duration::try_from_secs_f64(secs).ok())
            .and_then(|lasts| now.checked_add(lasts));
        Aura {
            expires,
            name: self.name,
            duration: self.duration,
            stacks: self.stacks.unwrap_or(1),
            value: self.value.and_then(|v| flex::float(v).ok()),
            category: self.category,
            persist: self.persist,
            caster,
            effects: self.effects,
        }
    }
}

#[derive(Deserialize)]
struct AuraName {
    #[serde(rename = "nam")]
    name: String,
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
                "auras": [{"nam": "Burn", "dur": 6, "isNew": true}]})),
            t0,
        );
        assert_eq!(auras.active[0].expires, Some(t0 + Duration::from_secs(6)));
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "m:1",
                "auras": [{"nam": "Burn", "dur": 10}]})),
            t0 + Duration::from_secs(1),
        );
        assert_eq!(auras.active[0].expires, Some(t0 + Duration::from_secs(11)));
        auras.apply(
            change(serde_json::json!({"cmd": "aura+", "cInf": "m:1",
                "auras": [{"nam": "Burn", "dur": 1e300}]})),
            t0,
        );
        assert_eq!(auras.active[0].expires, None);
    }
}
