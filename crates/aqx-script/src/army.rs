use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};

pub type Answer = Box<dyn FnOnce(Result<Value, String>) + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmyConfig {
    pub name: String,
    pub accounts: Vec<String>,
    pub room: Option<u32>,
}

#[derive(Clone, Default)]
pub struct Armies(Arc<Registry>);

#[derive(Default)]
struct Registry {
    configs: Vec<ArmyConfig>,
    live: Mutex<HashMap<String, Arc<Army>>>,
}

struct Army {
    config: ArmyConfig,
    state: Mutex<State>,
}

enum State {
    Forming(Vec<Option<Answer>>),
    Active(Option<Step>),
    Ended(String),
}

struct Step {
    label: String,
    arrived: Vec<Option<(Value, Answer)>>,
}

pub struct Member {
    army: Arc<Army>,
    seat: usize,
    left: bool,
}

#[derive(Serialize)]
struct Info<'a> {
    name: &'a str,
    number: usize,
    size: usize,
    room: Option<u32>,
    players: &'a [String],
}

type Fire = Vec<(Answer, Result<Value, String>)>;

fn fire(answers: Fire) {
    for (answer, result) in answers {
        answer(result);
    }
}

impl Armies {
    pub fn new(configs: Vec<ArmyConfig>) -> Armies {
        Armies(Arc::new(Registry {
            configs,
            live: Mutex::default(),
        }))
    }

    pub fn join(&self, name: &str, account: &str, answer: Answer) -> Result<Member, String> {
        let config = self
            .0
            .configs
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("aqx.toml has no army named {name}"))?;
        let seat = config
            .accounts
            .iter()
            .position(|a| a.eq_ignore_ascii_case(account))
            .ok_or_else(|| format!("{account} isn't in the {} army", config.name))?;
        let army = {
            let mut live = self.0.live.lock().unwrap_or_else(PoisonError::into_inner);
            let key = config.name.to_lowercase();
            let reusable = live
                .get(&key)
                .filter(|army| !matches!(*army.lock(), State::Ended(_)));
            match reusable {
                Some(army) => Arc::clone(army),
                None => {
                    let army = Arc::new(Army {
                        config: config.clone(),
                        state: Mutex::new(State::Forming(
                            config.accounts.iter().map(|_| None).collect(),
                        )),
                    });
                    live.insert(key, Arc::clone(&army));
                    army
                }
            }
        };
        let formed = {
            let mut state = army.lock();
            let State::Forming(seats) = &mut *state else {
                return Err(format!("the {} army is already running", config.name));
            };
            if seats[seat].is_some() {
                return Err(format!("{account} already joined the {} army", config.name));
            }
            seats[seat] = Some(answer);
            if seats.iter().all(Option::is_some) {
                let answers: Vec<Answer> = seats.drain(..).flatten().collect();
                *state = State::Active(None);
                answers
                    .into_iter()
                    .enumerate()
                    .map(|(seat, answer)| (answer, Ok(army.info(seat))))
                    .collect()
            } else {
                Vec::new()
            }
        };
        fire(formed);
        Ok(Member {
            army,
            seat,
            left: false,
        })
    }

    pub fn waiting(&self, account: &str) -> bool {
        let live = self.0.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.values().any(|army| {
            let Some(seat) = army
                .config
                .accounts
                .iter()
                .position(|a| a.eq_ignore_ascii_case(account))
            else {
                return false;
            };
            match &*army.lock() {
                State::Forming(seats) => seats[seat].is_some(),
                State::Active(Some(step)) => step.arrived[seat].is_some(),
                State::Active(None) | State::Ended(_) => false,
            }
        })
    }
}

impl Army {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn info(&self, seat: usize) -> Value {
        json!(Info {
            name: &self.config.name,
            number: seat + 1,
            size: self.config.accounts.len(),
            room: self.config.room,
            players: &self.config.accounts,
        })
    }

    fn player(&self, seat: usize) -> &str {
        &self.config.accounts[seat]
    }

    fn players(&self, seats: impl Iterator<Item = usize>) -> String {
        let names: Vec<&str> = seats.map(|seat| self.player(seat)).collect();
        names.join(", ")
    }

    fn end(&self, state: &mut State, reason: String) -> Fire {
        let ended = format!("the {} army ended: {reason}", self.config.name);
        let waiting: Vec<Answer> = match std::mem::replace(state, State::Ended(ended.clone())) {
            State::Forming(seats) => seats.into_iter().flatten().collect(),
            State::Active(Some(step)) => step
                .arrived
                .into_iter()
                .flatten()
                .map(|(_, answer)| answer)
                .collect(),
            State::Active(None) | State::Ended(_) => Vec::new(),
        };
        waiting
            .into_iter()
            .map(|answer| (answer, Err(ended.clone())))
            .collect()
    }
}

impl Member {
    pub fn arrive(&self, label: &str, value: Value, answer: Answer) {
        let army = &self.army;
        let me = army.player(self.seat);
        let fired = {
            let mut state = army.lock();
            match &mut *state {
                State::Ended(reason) => vec![(answer, Err(reason.clone()))],
                State::Forming(_) => vec![(answer, Err("the army hasn't formed yet".into()))],
                State::Active(step) => {
                    let step = step.get_or_insert_with(|| Step {
                        label: label.to_owned(),
                        arrived: army.config.accounts.iter().map(|_| None).collect(),
                    });
                    let clash = if step.label != label {
                        let there = army.players(
                            (0..step.arrived.len()).filter(|&s| step.arrived[s].is_some()),
                        );
                        Some(format!(
                            "{me} reached {label} while {there} waited at {}",
                            step.label
                        ))
                    } else if step.arrived[self.seat].is_some() {
                        Some(format!("{me} reached {label} twice"))
                    } else {
                        None
                    };
                    match clash {
                        Some(reason) => {
                            let mut fired = army.end(&mut state, reason);
                            let State::Ended(ended) = &*state else {
                                unreachable!("end leaves the army ended")
                            };
                            fired.push((answer, Err(ended.clone())));
                            fired
                        }
                        None => {
                            step.arrived[self.seat] = Some((value, answer));
                            if step.arrived.iter().all(Option::is_some) {
                                let (values, answers): (Vec<Value>, Vec<Answer>) =
                                    step.arrived.drain(..).flatten().unzip();
                                *state = State::Active(None);
                                let values = Value::Array(values);
                                answers
                                    .into_iter()
                                    .map(|answer| (answer, Ok(values.clone())))
                                    .collect()
                            } else {
                                Vec::new()
                            }
                        }
                    }
                }
            }
        };
        fire(fired);
    }

    pub fn leave(&mut self) {
        if std::mem::replace(&mut self.left, true) {
            return;
        }
        let army = &self.army;
        let me = army.player(self.seat);
        let fired = {
            let mut state = army.lock();
            match &mut *state {
                State::Ended(_) => Vec::new(),
                State::Forming(seats) => {
                    seats[self.seat] = None;
                    Vec::new()
                }
                State::Active(Some(step)) if step.arrived[self.seat].is_some() => {
                    let missing = army
                        .players((0..step.arrived.len()).filter(|&s| step.arrived[s].is_none()));
                    let reason = format!("{me} stopped waiting at {} for {missing}", step.label);
                    army.end(&mut state, reason)
                }
                State::Active(_) => army.end(&mut state, format!("{me} left")),
            }
        };
        fire(fired);
    }
}

impl Drop for Member {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn armies() -> Armies {
        Armies::new(vec![ArmyConfig {
            name: "ultras".into(),
            accounts: vec!["main".into(), "alt1".into(), "alt2".into()],
            room: Some(4821),
        }])
    }

    fn answer() -> (Answer, mpsc::Receiver<Result<Value, String>>) {
        let (tx, rx) = mpsc::channel();
        (
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
            rx,
        )
    }

    fn form(armies: &Armies) -> Vec<Member> {
        let joined: Vec<_> = ["main", "alt1", "alt2"]
            .iter()
            .map(|account| {
                let (answer, rx) = answer();
                (armies.join("Ultras", account, answer).unwrap(), rx)
            })
            .collect();
        joined
            .into_iter()
            .map(|(member, rx)| {
                rx.try_recv().unwrap().unwrap();
                member
            })
            .collect()
    }

    fn step(member: &Member, label: &str, value: Value) -> mpsc::Receiver<Result<Value, String>> {
        let (answer, rx) = answer();
        member.arrive(label, value, answer);
        rx
    }

    #[test]
    fn players_are_numbered_in_config_order_once_everyone_joins() {
        let armies = armies();
        let (first, joined_first) = answer();
        let _alt2 = armies.join("ultras", "ALT2", first).unwrap();
        assert!(
            joined_first.try_recv().is_err(),
            "the army waits for everyone"
        );
        assert!(armies.waiting("alt2"));
        let (second, joined_second) = answer();
        let _main = armies.join("ultras", "main", second).unwrap();
        let (third, _) = answer();
        let _alt1 = armies.join("ultras", "alt1", third).unwrap();
        assert_eq!(
            joined_first.try_recv().unwrap().unwrap(),
            json!({"name": "ultras", "number": 3, "size": 3, "room": 4821,
                   "players": ["main", "alt1", "alt2"]})
        );
        assert_eq!(joined_second.try_recv().unwrap().unwrap()["number"], 1);
        assert!(!armies.waiting("alt2"));
    }

    #[test]
    fn join_refuses_strangers_a_second_seat_and_a_running_army() {
        let armies = armies();
        let refused = |account: &str| armies.join("ultras", account, answer().0).err().unwrap();
        assert_eq!(refused("alt9"), "alt9 isn't in the ultras army");
        assert_eq!(
            armies.join("raid", "main", answer().0).err().unwrap(),
            "aqx.toml has no army named raid"
        );
        let first = armies.join("ultras", "main", answer().0).unwrap();
        assert_eq!(refused("main"), "main already joined the ultras army");
        drop(first);
        let _members = form(&armies);
        assert_eq!(refused("main"), "the ultras army is already running");
    }

    #[test]
    fn gather_answers_everyone_with_the_values_in_player_order() {
        let armies = armies();
        let [main, alt1, alt2] = form(&armies).try_into().ok().unwrap();
        let heard = [
            step(&alt2, "gather class", json!("LR")),
            step(&main, "gather class", json!("LOO")),
        ];
        assert!(heard[0].try_recv().is_err(), "a step waits for everyone");
        assert!(armies.waiting("alt2") && !armies.waiting("alt1"));
        let last = step(&alt1, "gather class", json!("AP"));
        for rx in heard.iter().chain([&last]) {
            assert_eq!(rx.try_recv().unwrap().unwrap(), json!(["LOO", "AP", "LR"]));
        }
        let again = [
            step(&main, "gather class", json!(1)),
            step(&alt1, "gather class", json!(2)),
            step(&alt2, "gather class", json!(3)),
        ];
        assert_eq!(again[0].try_recv().unwrap().unwrap(), json!([1, 2, 3]));
    }

    #[test]
    fn players_at_different_steps_end_the_army() {
        let armies = armies();
        let [main, alt1, alt2] = form(&armies).try_into().ok().unwrap();
        let waiting = step(&main, "sync boss", Value::Null);
        let lost = step(&alt1, "sync loot", Value::Null);
        let ended = "the ultras army ended: alt1 reached sync loot while main waited at sync boss";
        assert_eq!(waiting.try_recv().unwrap().unwrap_err(), ended);
        assert_eq!(lost.try_recv().unwrap().unwrap_err(), ended);
        let late = step(&alt2, "sync boss", Value::Null);
        assert_eq!(late.try_recv().unwrap().unwrap_err(), ended);
    }

    #[test]
    fn a_member_that_leaves_ends_the_army_naming_who_it_waited_for() {
        let armies = armies();
        let [mut main, alt1, alt2] = form(&armies).try_into().ok().unwrap();
        let waiting = step(&alt1, "sync boss", Value::Null);
        let quitting = step(&main, "sync boss", Value::Null);
        main.leave();
        let ended = "the ultras army ended: main stopped waiting at sync boss for alt2";
        assert_eq!(waiting.try_recv().unwrap().unwrap_err(), ended);
        assert_eq!(quitting.try_recv().unwrap().unwrap_err(), ended);
        drop((alt1, alt2));
        let [main, alt1, alt2] = form(&armies).try_into().ok().unwrap();
        let waiting = step(&main, "sync boss", Value::Null);
        drop(alt2);
        assert_eq!(
            waiting.try_recv().unwrap().unwrap_err(),
            "the ultras army ended: alt2 left"
        );
        drop((main, alt1));
    }

    #[test]
    fn leaving_before_the_army_forms_frees_the_seat() {
        let armies = armies();
        let (first, joined) = answer();
        let early = armies.join("ultras", "main", first).unwrap();
        drop(early);
        assert!(!armies.waiting("main"));
        let members = form(&armies);
        assert_eq!(members.len(), 3);
        assert!(
            joined.try_recv().is_err(),
            "the member that left hears nothing"
        );
    }
}
