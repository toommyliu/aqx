use std::collections::HashMap;

use mira_protocol::{ItemId, flex};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Enhancement {
    pub id: i64,
    pub level: u32,
    pub pattern: u32,
    pub proc_id: u32,
    pub plus: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Forge,
}

const PATTERNS: [(u32, &str, Kind); 21] = [
    (1, "Adventurer", Kind::Plain),
    (2, "Fighter", Kind::Plain),
    (3, "Thief", Kind::Plain),
    (4, "Armsman", Kind::Plain),
    (5, "Hybrid", Kind::Plain),
    (6, "Wizard", Kind::Plain),
    (7, "Healer", Kind::Plain),
    (8, "Spellbreaker", Kind::Plain),
    (9, "Lucky", Kind::Plain),
    (23, "Depths", Kind::Plain),
    (10, "Forge", Kind::Plain),
    (11, "Absolution", Kind::Forge),
    (12, "Avarice", Kind::Forge),
    (24, "Vainglory", Kind::Forge),
    (25, "Vim", Kind::Forge),
    (26, "Examen", Kind::Forge),
    (27, "Pneuma", Kind::Forge),
    (28, "Anima", Kind::Forge),
    (29, "Penitence", Kind::Forge),
    (30, "Lament", Kind::Forge),
    (32, "Hearty", Kind::Forge),
];

const PROCS: [&str; 14] = [
    "Spiral Carve",
    "Awe Blast",
    "Health Vamp",
    "Mana Vamp",
    "Powerword DIE",
    "Lacerate",
    "Smite",
    "Valiance",
    "Arcana's Concerto",
    "Acheron",
    "Elysium",
    "Praxis",
    "Dauntless",
    "Ravenous",
];

const FIRST_PROC: u32 = 2;

impl Enhancement {
    pub fn name(&self, server_procs: &HashMap<u32, String>) -> Option<String> {
        let &(_, pattern, kind) = PATTERNS.iter().find(|(id, ..)| *id == self.pattern)?;
        let mut name = match (kind, pattern) {
            (Kind::Plain, _) | (Kind::Forge, "Vim" | "Examen" | "Pneuma" | "Anima") => {
                pattern.to_owned()
            }
            (Kind::Forge, "Hearty") => "Grimskull".to_owned(),
            (Kind::Forge, _) => "Forge".to_owned(),
        };
        if self.plus > 0 {
            name.push_str(&format!(" +{}", self.plus));
        }
        let special = match kind {
            Kind::Plain if self.proc_id > 0 => {
                let known = self
                    .proc_id
                    .checked_sub(FIRST_PROC)
                    .and_then(|at| PROCS.get(at as usize));
                let named = server_procs.get(&self.proc_id).map(String::as_str);
                Some(named.or(known.copied()).unwrap_or("Unknown"))
            }
            Kind::Plain => None,
            Kind::Forge => Some(match pattern {
                "Vim" | "Examen" => "Ether",
                "Pneuma" | "Anima" => "Clairvoyance",
                _ => pattern,
            }),
        };
        if let Some(special) = special {
            name.push_str(", ");
            name.push_str(special);
        }
        Some(name)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct EnhancementFields {
    #[serde(rename = "EnhID", default, deserialize_with = "flex::opt_int")]
    id: Option<i64>,
    #[serde(rename = "EnhLvl", default, deserialize_with = "flex::opt_int")]
    level: Option<u32>,
    #[serde(rename = "PatternID", default, deserialize_with = "flex::opt_int")]
    item_pattern: Option<u32>,
    #[serde(
        rename = "EnhPatternID",
        alias = "EnhPID",
        default,
        deserialize_with = "flex::opt_int"
    )]
    pattern: Option<u32>,
    #[serde(rename = "ProcID", default, deserialize_with = "flex::opt_int")]
    proc_id: Option<u32>,
    #[serde(rename = "EnhRty", default, deserialize_with = "flex::opt_int")]
    rarity: Option<u32>,
    #[serde(rename = "iRty", default, deserialize_with = "flex::opt_int")]
    item_rarity: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct Enhanced {
    #[serde(rename = "ItemIDs")]
    pub items: Vec<ItemId>,
    #[serde(flatten)]
    pub fields: EnhancementFields,
    #[serde(rename = "iCost", default, deserialize_with = "flex::int")]
    pub cost: u64,
}

impl EnhancementFields {
    pub(crate) fn or(self, fallback: EnhancementFields) -> EnhancementFields {
        EnhancementFields {
            id: self.id.or(fallback.id),
            level: self.level.or(fallback.level),
            item_pattern: self.item_pattern.or(fallback.item_pattern),
            pattern: self.pattern.or(fallback.pattern),
            proc_id: self.proc_id.or(fallback.proc_id),
            rarity: self.rarity.or(fallback.rarity),
            item_rarity: self.item_rarity.or(fallback.item_rarity),
        }
    }

    pub(crate) fn enhancement(&self) -> Option<Enhancement> {
        let id = self.id.filter(|id| *id > 0)?;
        let plus = match (self.rarity, self.item_rarity) {
            (Some(rarity), _) if rarity > 1 => rarity - 1,
            (_, Some(rarity)) if (2..10).contains(&rarity) => rarity - 1,
            _ => 0,
        };
        Some(Enhancement {
            id,
            level: self.level.unwrap_or(0),
            pattern: self.pattern.or(self.item_pattern).unwrap_or(0),
            proc_id: self.proc_id.unwrap_or(0),
            plus,
        })
    }

    pub(crate) fn from_id(id: i64) -> EnhancementFields {
        EnhancementFields {
            id: Some(id),
            ..EnhancementFields::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(pattern: u32, proc_id: u32, plus: u32) -> Option<String> {
        let enhancement = Enhancement {
            id: 1,
            level: 1,
            pattern,
            proc_id,
            plus,
        };
        enhancement.name(&HashMap::new())
    }

    #[test]
    fn names_follow_the_game_clients_enhancement_line() {
        assert_eq!(named(1, 0, 0).as_deref(), Some("Adventurer"));
        assert_eq!(named(9, 2, 0).as_deref(), Some("Lucky, Spiral Carve"));
        assert_eq!(named(10, 15, 0).as_deref(), Some("Forge, Ravenous"));
        assert_eq!(named(6, 99, 0).as_deref(), Some("Wizard, Unknown"));
        assert_eq!(named(11, 0, 0).as_deref(), Some("Forge, Absolution"));
        assert_eq!(named(11, 3, 0).as_deref(), Some("Forge, Absolution"));
        assert_eq!(named(25, 0, 0).as_deref(), Some("Vim, Ether"));
        assert_eq!(named(28, 0, 0).as_deref(), Some("Anima, Clairvoyance"));
        assert_eq!(named(32, 0, 0).as_deref(), Some("Grimskull, Hearty"));
        assert_eq!(named(2, 0, 2).as_deref(), Some("Fighter +2"));
        assert_eq!(named(31, 0, 0), None);
        let server = HashMap::from([(2, "Spiral Carve II".to_owned())]);
        let carve = Enhancement {
            id: 1,
            level: 1,
            pattern: 9,
            proc_id: 2,
            plus: 0,
        };
        assert_eq!(
            carve.name(&server).as_deref(),
            Some("Lucky, Spiral Carve II")
        );
    }

    #[test]
    fn the_plus_comes_from_the_enhancement_rarity_then_the_items() {
        let plus = |json: &str| {
            serde_json::from_str::<EnhancementFields>(json)
                .unwrap()
                .enhancement()
                .map(|e| e.plus)
        };
        assert_eq!(plus(r#"{"EnhID": 5, "EnhRty": 3, "iRty": 4}"#), Some(2));
        assert_eq!(plus(r#"{"EnhID": 5, "EnhRty": 1, "iRty": 4}"#), Some(3));
        assert_eq!(plus(r#"{"EnhID": 5, "iRty": 12}"#), Some(0));
        assert_eq!(plus(r#"{"EnhID": 0, "EnhRty": 3}"#), None);
        let pattern = serde_json::from_str::<EnhancementFields>(
            r#"{"EnhID": 5, "PatternID": 2, "EnhPatternID": "9"}"#,
        )
        .unwrap()
        .enhancement()
        .map(|e| e.pattern);
        assert_eq!(pattern, Some(9));
    }
}
