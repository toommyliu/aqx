use std::task::Poll;
use std::time::Instant;

use mira_protocol::{ItemId, QuestId, packet};
use serde::Deserialize;

use super::{ActionError, Limit, Op, Session};
use crate::REPLY_TIMEOUT;
use crate::quest::{self, Held, Quest, QuestStatus, RewardKind};

const LOAD_TRIES: u32 = 5;

pub(super) enum QuestAction {
    Load(Vec<QuestId>),
    Accept {
        id: QuestId,
        checked: bool,
    },
    TurnIn {
        id: QuestId,
        reward: Option<ItemId>,
        times: i64,
        special: bool,
        checked: bool,
    },
}

pub(super) struct QuestCall {
    action: QuestAction,
    unfetched: Vec<QuestId>,
    slowed: u32,
    pub(super) stage: Stage,
}

pub(super) enum Stage {
    Ready {
        wake: Option<Instant>,
    },
    Fetching {
        batch: Vec<QuestId>,
        deadline: Instant,
    },
    Sent {
        deadline: Instant,
    },
}

impl Stage {
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            Stage::Ready { wake } => *wake,
            Stage::Fetching { deadline, .. } | Stage::Sent { deadline } => Some(*deadline),
        }
    }

    pub(super) fn awaiting(&self) -> bool {
        !matches!(self, Stage::Ready { .. })
    }
}

pub(super) enum Answer {
    Quests(Vec<QuestId>),
    SlowedDown,
    Accepted(Verdict),
    TurnedIn(Verdict),
    TurnInEchoed(QuestId),
}

#[derive(Deserialize)]
pub(super) struct Verdict {
    #[serde(rename = "QuestID")]
    pub(super) id: QuestId,
    #[serde(rename = "bSuccess", deserialize_with = "mira_protocol::flex::bool")]
    pub(super) ok: bool,
    #[serde(default)]
    msg: Option<String>,
}

impl Verdict {
    fn result(self, failed: &str) -> Result<(), ActionError> {
        if self.ok {
            return Ok(());
        }
        let why = self.msg.filter(|m| !m.is_empty());
        Err(ActionError::Refused(
            why.unwrap_or_else(|| failed.to_owned()),
        ))
    }
}

impl Session {
    pub(super) fn start_quest(&self, action: QuestAction) -> QuestCall {
        let mut unfetched: Vec<QuestId> = match &action {
            QuestAction::Load(ids) => ids.clone(),
            QuestAction::TurnIn { checked: false, .. } => Vec::new(),
            QuestAction::Accept { id, .. } | QuestAction::TurnIn { id, .. } => vec![*id],
        };
        unfetched.retain(|id| !self.quests.contains_key(id));
        unfetched.sort();
        unfetched.dedup();
        QuestCall {
            action,
            unfetched,
            slowed: 0,
            stage: Stage::Ready { wake: None },
        }
    }

    pub(super) fn step_quest(
        &mut self,
        call: &mut QuestCall,
        now: Instant,
    ) -> Poll<Result<(), ActionError>> {
        if let Err(e) = self.ensure_ready() {
            return Poll::Ready(Err(e));
        }
        if let Stage::Fetching { deadline, .. } | Stage::Sent { deadline } = call.stage {
            return if now >= deadline {
                Poll::Ready(Err(ActionError::NoReply))
            } else {
                Poll::Pending
            };
        }
        let Some(area) = self.room.as_ref().map(|room| room.area) else {
            return Poll::Ready(Err(ActionError::NotReady));
        };
        if !call.unfetched.is_empty() {
            if let Some(until) = self.cooling(Limit::LoadQuests, now) {
                call.stage = Stage::Ready { wake: Some(until) };
                return Poll::Pending;
            }
            let batch = std::mem::take(&mut call.unfetched);
            self.limits.used(Limit::LoadQuests, now);
            self.send(&packet::get_quests(area, &batch));
            call.stage = Stage::Fetching {
                batch,
                deadline: now + REPLY_TIMEOUT,
            };
            return Poll::Pending;
        }
        let (limit, packet) = match &call.action {
            QuestAction::Load(ids) => {
                return Poll::Ready(match ids.iter().find(|id| !self.quests.contains_key(id)) {
                    Some(id) => Err(ActionError::NoTarget(format!(
                        "the server has no quest {id}"
                    ))),
                    None => Ok(()),
                });
            }
            &QuestAction::Accept { id, checked } => {
                if self.accepted_quests.contains(&id) {
                    return Poll::Ready(Ok(()));
                }
                let found = self.quest(id);
                let checks = found.and_then(|quest| {
                    if checked {
                        self.acceptable(quest)
                    } else {
                        Ok(())
                    }
                });
                if let Err(e) = checks {
                    return Poll::Ready(Err(e));
                }
                (Limit::AcceptQuest, packet::accept_quest(area, id))
            }
            &QuestAction::TurnIn {
                id,
                reward,
                times,
                special,
                checked,
            } => {
                if checked
                    && let Err(e) = self
                        .quest(id)
                        .and_then(|quest| self.can_turn_in(quest, reward, times))
                {
                    return Poll::Ready(Err(e));
                }
                (
                    Limit::TurnIn,
                    packet::try_quest_complete_special(area, id, reward, times, special),
                )
            }
        };
        if let Some(until) = self.cooling(limit, now) {
            call.stage = Stage::Ready { wake: Some(until) };
            return Poll::Pending;
        }
        self.limits.used(limit, now);
        self.send(&packet);
        call.stage = Stage::Sent {
            deadline: now + REPLY_TIMEOUT,
        };
        Poll::Pending
    }

    pub(super) fn answer(&mut self, answer: Answer) {
        let Some(Op::Quest { call, .. }) = &mut self.current else {
            return;
        };
        let result = match (&mut call.stage, &call.action, answer) {
            (Stage::Fetching { batch, .. }, _, Answer::Quests(ids))
                if ids.iter().all(|id| batch.contains(id)) =>
            {
                call.slowed = 0;
                call.stage = Stage::Ready { wake: None };
                return;
            }
            (Stage::Fetching { batch, .. }, _, Answer::SlowedDown)
                if call.slowed + 1 < LOAD_TRIES =>
            {
                call.unfetched.splice(0..0, std::mem::take(batch));
                call.slowed += 1;
                call.stage = Stage::Ready { wake: None };
                return;
            }
            (Stage::Fetching { .. }, _, Answer::SlowedDown) => Err(ActionError::Refused(
                "the server kept saying to slow down while loading quests".into(),
            )),
            (Stage::Sent { .. }, QuestAction::Accept { id, .. }, Answer::Accepted(verdict))
                if verdict.id == *id =>
            {
                verdict.result("the server refused the quest")
            }
            (Stage::Sent { .. }, QuestAction::TurnIn { id, .. }, Answer::TurnedIn(verdict))
                if verdict.id == *id =>
            {
                verdict.result("the turn-in failed")
            }
            (Stage::Sent { .. }, QuestAction::TurnIn { id, .. }, Answer::TurnInEchoed(echoed))
                if echoed == *id =>
            {
                Err(ActionError::Refused(
                    "the server sent the turn-in back unanswered".into(),
                ))
            }
            _ => return,
        };
        if let Some(Op::Quest { reply, .. }) = self.current.take() {
            self.replies.push((reply, result));
        }
    }

    fn quest(&self, id: QuestId) -> Result<&Quest, ActionError> {
        self.quests
            .get(&id)
            .ok_or_else(|| ActionError::NoTarget(format!("the server has no quest {id}")))
    }

    fn held(&self) -> Held<'_> {
        Held {
            items: &self.inventory.items,
            temp: &self.inventory.temp,
        }
    }

    fn acceptable(&self, quest: &Quest) -> Result<(), ActionError> {
        let Some(me) = &self.character else {
            return Err(ActionError::NotReady);
        };
        let why = match quest::status(quest, me, self.held(), &self.factions) {
            QuestStatus::Available => return Ok(()),
            QuestStatus::Done => "is done and can't be repeated",
            QuestStatus::DoneThisPeriod => "is done until it resets",
            QuestStatus::Locked => "is locked",
        };
        Err(ActionError::Unusable(format!("{} {why}", quest.name)))
    }

    fn can_turn_in(
        &self,
        quest: &Quest,
        reward: Option<ItemId>,
        times: i64,
    ) -> Result<(), ActionError> {
        let unusable = |why: String| Err(ActionError::Unusable(why));
        let accepted = self.accepted_quests.contains(&quest.id);
        let most =
            quest::turn_in_limit(quest, accepted, self.held()).map_err(ActionError::Unusable)?;
        let mut choices = quest
            .rewards
            .iter()
            .filter(|r| r.kind == RewardKind::Chosen);
        match reward {
            None if choices.next().is_some() => {
                return unusable(format!("choose one of {}'s rewards", quest.name));
            }
            Some(id) if !choices.any(|r| r.id == id) => {
                return unusable(format!("item {id} isn't a reward {} offers", quest.name));
            }
            _ => {}
        }
        if times < 1 || times > i64::from(most) {
            return unusable(format!(
                "{} can be turned in {most} time{} at once now",
                quest.name,
                if most == 1 { "" } else { "s" }
            ));
        }
        Ok(())
    }
}
