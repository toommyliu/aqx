use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use mio::Waker;
use serde_json::{Map, Value};

use crate::actions::ActError;
use crate::control::StopHandle;
use crate::session::{Halt, Phase, Session};
use crate::world::{Aura, State, World};

pub struct Team {
    name: Option<String>,
    board: Mutex<Board>,
}

struct Board {
    slots: Vec<Slot>,
    round: u64,
    released: Vec<Value>,
    failed: Option<String>,
}

struct Slot {
    name: String,
    waker: Arc<Waker>,
    live: Live,
    shared: Map<String, Value>,
    arrived: Option<(String, Value)>,
    returned: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub online: bool,
    pub map: String,
    pub cell: String,
    pub state: State,
    pub hp: u32,
    pub max_hp: u32,
    pub class: String,
    pub auras: Vec<Aura>,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub index: usize,
    pub live: Live,
    pub shared: Map<String, Value>,
}

pub struct Waiting {
    pub label: String,
    pub missing: Vec<String>,
}

#[derive(Clone)]
pub struct Membership {
    team: Arc<Team>,
    index: usize,
}

impl Membership {
    pub fn team(&self) -> &Arc<Team> {
        &self.team
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

fn returned_before(name: &str, label: &str) -> String {
    format!("{name}'s script returned before {label:?}")
}

impl Team {
    pub fn new(name: Option<String>, members: Vec<(String, StopHandle)>) -> Arc<Team> {
        let slots = members
            .into_iter()
            .map(|(name, stop)| Slot { name, waker: stop.waker(), live: Live::default(), shared: Map::new(), arrived: None, returned: false })
            .collect();
        Arc::new(Team { name, board: Mutex::new(Board { slots, round: 0, released: Vec::new(), failed: None }) })
    }

    pub fn memberships(self: &Arc<Self>) -> Vec<Membership> {
        (0..self.board().slots.len()).map(|index| Membership { team: self.clone(), index }).collect()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn board(&self) -> MutexGuard<'_, Board> {
        self.board.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn members(&self) -> Vec<Member> {
        let board = self.board();
        board
            .slots
            .iter()
            .enumerate()
            .map(|(index, s)| Member { name: s.name.clone(), index, live: s.live.clone(), shared: s.shared.clone() })
            .collect()
    }

    pub fn waiting(&self) -> Option<Waiting> {
        let board = self.board();
        let label = board.pending()?.to_string();
        let missing = board.missing();
        Some(Waiting { label, missing })
    }

    fn share(&self, index: usize, patch: Map<String, Value>) {
        let mut board = self.board();
        board.slots[index].shared.extend(patch);
        board.wake_others(index);
    }

    fn finish(&self, index: usize) {
        let mut board = self.board();
        board.slots[index].returned = true;
        if board.slots[index].arrived.is_none()
            && let Some(label) = board.pending()
        {
            let failure = returned_before(&board.slots[index].name, label);
            board.fail(failure);
        }
    }

    fn publish(&self, index: usize, live: Live) {
        let mut board = self.board();
        if board.slots[index].live != live {
            board.slots[index].live = live;
            board.wake_others(index);
        }
    }

    fn arrive(&self, index: usize, label: &str, value: Value) -> Result<u64, String> {
        let mut board = self.board();
        if let Some(failure) = &board.failed {
            return Err(failure.clone());
        }
        if let Some(at) = board.pending().filter(|at| *at != label) {
            let other = board.slots.iter().find(|s| s.arrived.is_some()).map_or("", |s| s.name.as_str());
            let failure = format!("{} reached {label:?} while {other} waits at {at:?}", board.slots[index].name);
            return Err(board.fail(failure));
        }
        if let Some(gone) = board.slots.iter().find(|s| s.returned && s.arrived.is_none()) {
            let failure = returned_before(&gone.name, label);
            return Err(board.fail(failure));
        }
        let round = board.round;
        board.slots[index].arrived = Some((label.to_string(), value));
        if board.slots.iter().all(|s| s.arrived.is_some()) {
            board.released = board.slots.iter_mut().map(|s| s.arrived.take().map(|(_, v)| v).unwrap_or_default()).collect();
            board.round += 1;
            board.wake_others(index);
        }
        Ok(round)
    }

    fn released(&self, round: u64) -> Option<Result<Vec<Value>, String>> {
        let board = self.board();
        if board.round > round {
            return Some(Ok(board.released.clone()));
        }
        board.failed.clone().map(Err)
    }

    fn give_up(&self, index: usize, round: u64, label: &str) -> Result<Vec<Value>, String> {
        let mut board = self.board();
        if board.round > round {
            return Ok(board.released.clone());
        }
        let missing = board.missing().join(", ");
        let failure = format!("{} stopped waiting at {label:?} for {missing}", board.slots[index].name);
        board.fail(failure);
        Err(format!("waiting for {missing} at {label:?}"))
    }

    fn withdraw(&self, index: usize, round: u64) {
        let mut board = self.board();
        if board.round == round {
            board.slots[index].arrived = None;
        }
    }
}

impl Board {
    fn pending(&self) -> Option<&str> {
        self.slots.iter().find_map(|s| s.arrived.as_ref().map(|(label, _)| label.as_str()))
    }

    fn missing(&self) -> Vec<String> {
        self.slots.iter().filter(|s| s.arrived.is_none()).map(|s| s.name.clone()).collect()
    }

    fn wake_others(&self, index: usize) {
        for (i, slot) in self.slots.iter().enumerate() {
            if i != index {
                let _ = slot.waker.wake();
            }
        }
    }

    fn fail(&mut self, failure: String) -> String {
        self.failed = Some(failure.clone());
        for slot in &self.slots {
            let _ = slot.waker.wake();
        }
        failure
    }
}

pub(crate) fn live(phase: &Phase, world: &World) -> Live {
    let Some(me) = &world.me else { return Live::default() };
    Live {
        online: *phase == Phase::Ready,
        map: world.area.as_ref().map(|a| a.map.clone()).unwrap_or_default(),
        cell: me.cell.clone(),
        state: me.vitals.state,
        hp: me.vitals.hp,
        max_hp: me.vitals.max_hp,
        class: me.class.clone(),
        auras: me.auras.clone(),
    }
}

impl Session {
    pub fn set_team(&mut self, team: Option<Membership>) {
        if let Some(Membership { team, index }) = self.team.take() {
            team.finish(index);
        }
        self.team = team;
        self.team_seen = None;
        self.publish_member();
    }

    pub fn team(&self) -> Option<&Membership> {
        self.team.as_ref()
    }

    pub(crate) fn publish_member(&mut self) {
        let Some(Membership { team, index }) = &self.team else { return };
        let seen = (self.world.rev, self.phase == Phase::Ready);
        if self.team_seen != Some(seen) {
            self.team_seen = Some(seen);
            team.publish(*index, live(&self.phase, &self.world));
        }
    }

    pub fn sync(&mut self, label: &str, value: Value, deadline: Option<Instant>) -> Result<Vec<Value>, ActError> {
        let Some(Membership { team, index }) = self.team.clone() else { return Ok(vec![value]) };
        let round = team.arrive(index, label, value).map_err(ActError::Rejected)?;
        match self.pump_until(deadline, |_| team.released(round)) {
            Ok(released) => released.map_err(ActError::Rejected),
            Err(Halt::Timeout) => team.give_up(index, round, label).map_err(ActError::Timeout),
            Err(halt) => {
                team.withdraw(index, round);
                Err(ActError::Halted(halt))
            }
        }
    }

    pub fn share(&mut self, patch: Map<String, Value>) {
        if let Some(Membership { team, index }) = &self.team {
            team.share(*index, patch);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::control::Control;

    fn team(names: &[&str]) -> (Arc<Team>, Vec<Control>) {
        let controls: Vec<Control> = names.iter().map(|_| Control::new().unwrap()).collect();
        let members = names.iter().zip(&controls).map(|(n, c)| (n.to_string(), c.stop_handle())).collect();
        (Team::new(Some("ultras".into()), members), controls)
    }

    #[test]
    fn the_last_arrival_releases_every_value_in_member_order() {
        let (team, _controls) = team(&["a", "b", "c"]);
        let round = team.arrive(2, "ready", json!("c")).unwrap();
        assert!(team.released(round).is_none());
        team.arrive(0, "ready", json!("a")).unwrap();
        assert_eq!(team.waiting().map(|w| (w.label, w.missing)), Some(("ready".into(), vec!["b".into()])));
        team.arrive(1, "ready", json!("b")).unwrap();
        assert_eq!(team.released(round), Some(Ok(vec![json!("a"), json!("b"), json!("c")])));
        assert!(team.waiting().is_none());
        assert!(team.released(round + 1).is_none(), "the next barrier starts empty");
    }

    #[test]
    fn a_different_label_fails_the_barrier_for_everyone() {
        let (team, _controls) = team(&["a", "b"]);
        let round = team.arrive(0, "buffed", Value::Null).unwrap();
        let failure = r#"b reached "join" while a waits at "buffed""#;
        assert_eq!(team.arrive(1, "join", Value::Null), Err(failure.to_string()));
        assert_eq!(team.released(round), Some(Err(failure.to_string())));
        assert_eq!(team.arrive(1, "buffed", Value::Null), Err(failure.to_string()), "a failed team stays failed");
    }

    #[test]
    fn a_returned_member_fails_the_barrier_it_never_reaches() {
        let (waiting, _controls) = team(&["a", "b"]);
        let round = waiting.arrive(0, "loot", Value::Null).unwrap();
        waiting.finish(1);
        assert_eq!(waiting.released(round), Some(Err(r#"b's script returned before "loot""#.to_string())));

        let (later, _controls) = team(&["a", "b"]);
        later.finish(1);
        assert_eq!(later.arrive(0, "loot", Value::Null), Err(r#"b's script returned before "loot""#.to_string()));
    }

    #[test]
    fn returning_after_the_last_barrier_fails_nothing() {
        let (team, _controls) = team(&["a", "b"]);
        let round = team.arrive(0, "done", Value::Null).unwrap();
        team.arrive(1, "done", Value::Null).unwrap();
        team.finish(1);
        team.finish(0);
        assert_eq!(team.released(round), Some(Ok(vec![Value::Null, Value::Null])));
    }

    #[test]
    fn giving_up_fails_the_barrier_for_everyone_unless_it_was_already_released() {
        let (three, _controls) = team(&["a", "b", "c"]);
        let round = three.arrive(0, "early", Value::Null).unwrap();
        three.arrive(1, "early", Value::Null).unwrap();
        assert_eq!(three.give_up(0, round, "early"), Err(r#"waiting for c at "early""#.to_string()));
        assert_eq!(three.released(round), Some(Err(r#"a stopped waiting at "early" for c"#.to_string())));

        let (raced, _controls) = team(&["a", "b"]);
        let round = raced.arrive(0, "ready", json!(1)).unwrap();
        raced.arrive(1, "ready", json!(2)).unwrap();
        assert_eq!(raced.give_up(0, round, "ready"), Ok(vec![json!(1), json!(2)]), "the release won the race");
    }

    #[test]
    fn a_withdrawn_arrival_frees_the_barrier_for_another_label() {
        let (team, _controls) = team(&["a", "b"]);
        let round = team.arrive(0, "first", Value::Null).unwrap();
        team.withdraw(0, round);
        let round = team.arrive(1, "second", Value::Null).unwrap();
        team.arrive(0, "second", Value::Null).unwrap();
        assert_eq!(team.released(round), Some(Ok(vec![Value::Null, Value::Null])));
    }

    #[test]
    fn share_merges_into_the_callers_record_only() {
        let (team, _controls) = team(&["a", "b"]);
        team.share(0, json!({ "role": "LR", "ready": false }).as_object().unwrap().clone());
        team.share(0, json!({ "ready": true }).as_object().unwrap().clone());
        let members = team.members();
        assert_eq!(Value::Object(members[0].shared.clone()), json!({ "role": "LR", "ready": true }));
        assert!(members[1].shared.is_empty());
    }
}
