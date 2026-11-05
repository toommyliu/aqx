use serde::Serialize;

use crate::session::Phase;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Phase(Phase),
    Joined {
        map: String,
        cell: String,
        pad: String,
        players: usize,
    },
    Moved {
        cell: String,
        pad: String,
    },
    PlayerEntered {
        name: String,
    },
    PlayerLeft {
        name: String,
    },
    Ready {
        level: u32,
        gold: u64,
        items: usize,
    },
    Chat {
        channel: String,
        sender: String,
        text: String,
    },
    Whisper {
        sender: String,
        text: String,
    },
    Notice {
        kind: NoticeKind,
        text: String,
    },
    Unreadable {
        error: String,
    },
    Packet {
        direction: Direction,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Server,
    Notify,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
}
