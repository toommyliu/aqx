use std::sync::Arc;
use std::thread;

use std::time::Instant;

use super::{CounterHold, Disconnect, Session};
use crate::api::{self, ApiError};

pub(super) struct Recovery {
    pub(super) why: Disconnect,
    pub(super) step: Step,
}

pub(super) enum Step {
    Waiting(Instant),
    Relogging,
    Rejoining,
}

impl Session {
    pub(super) fn forget_session(&mut self) {
        self.counter_hold = CounterHold::Off;
        self.aggro = None;
        self.anti_afk = false;
        self.afk_cleared = None;
        self.requested_spot = None;
        self.shop_maps.clear();
        self.procs = Arc::default();
    }

    pub(super) fn relogin(&mut self) {
        let Some(password) = self.options.reconnect.as_ref().map(|p| p.password.clone()) else {
            return;
        };
        let (base, device) = (self.account.api.clone(), self.account.device.clone());
        let username = self.account.username.clone();
        let (results, waker) = (self.relogin_out.clone(), Arc::clone(&self.waker));
        thread::spawn(move || {
            let _ = results.send(api::login_at(&base, &device, &username, &password));
            let _ = waker.wake();
        });
    }

    pub(super) fn take_relogin_results(&mut self) {
        while let Ok(result) = self.relogin_results.try_recv() {
            if !matches!(
                self.recovery,
                Some(Recovery {
                    step: Step::Relogging,
                    ..
                })
            ) {
                continue;
            }
            let account = match result {
                Ok(account) => account,
                Err(ApiError::Rejected(why)) => {
                    self.close(Disconnect::Rejected(why));
                    continue;
                }
                Err(e) => {
                    self.close(Disconnect::ConnectionLost(e.to_string()));
                    continue;
                }
            };
            let server = match account.pick_server(&self.server.name) {
                Ok(server) => server.clone(),
                Err(e) => {
                    self.close(Disconnect::ConnectionLost(e.to_string()));
                    continue;
                }
            };
            self.token = account.token.clone();
            self.account = account;
            if let Some(recovery) = &mut self.recovery {
                recovery.step = Step::Rejoining;
            }
            let version = self.version.clone();
            self.current = Some(self.start_login(server, version, 1));
        }
    }
}
