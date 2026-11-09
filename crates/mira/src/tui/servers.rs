use std::sync::mpsc::{Receiver, TryRecvError};

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::control::ServerChoice;

pub enum Listing {
    Loading(Receiver<Result<Vec<ServerChoice>, String>>),
    Failed(String),
    Ready(Vec<ServerChoice>),
}

pub struct Servers {
    pub current: String,
    pub listing: Listing,
    pub cursor: usize,
    pub refused: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Switch(String),
    Nothing,
}

impl Servers {
    pub fn open(current: String, listed: Receiver<Result<Vec<ServerChoice>, String>>) -> Servers {
        let mut servers = Servers {
            current,
            listing: Listing::Loading(listed),
            cursor: 0,
            refused: None,
        };
        servers.poll();
        servers
    }

    pub fn poll(&mut self) {
        let Listing::Loading(listed) = &self.listing else {
            return;
        };
        self.listing = match listed.try_recv() {
            Ok(Ok(servers)) => {
                self.cursor = servers
                    .iter()
                    .position(|s| s.name.eq_ignore_ascii_case(&self.current))
                    .unwrap_or(0);
                Listing::Ready(servers)
            }
            Ok(Err(why)) => Listing::Failed(why),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Listing::Failed("the server list didn't arrive".into())
            }
        };
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let Listing::Ready(servers) = &self.listing else {
            return match key.code {
                KeyCode::Esc | KeyCode::Enter => Outcome::Close,
                _ => Outcome::Nothing,
            };
        };
        let last = servers.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Outcome::Close,
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down => self.cursor = (self.cursor + 1).min(last),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = last,
            KeyCode::Enter => {
                let Some(server) = servers.get(self.cursor) else {
                    return Outcome::Nothing;
                };
                match &server.refusal {
                    Some(why) => self.refused = Some(format!("{} {why}", server.name)),
                    None => return Outcome::Switch(server.name.clone()),
                }
                return Outcome::Nothing;
            }
            _ => return Outcome::Nothing,
        }
        self.refused = None;
        Outcome::Nothing
    }
}
