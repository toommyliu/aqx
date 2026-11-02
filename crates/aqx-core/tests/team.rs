mod server;

use std::rc::Rc;
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aqx_core::{ActError, Control, Membership, Session, Team};
use serde_json::{Value, json};
use server::{FARM, Servers};

const PATIENCE: Duration = Duration::from_secs(5);

struct Members {
    seats: Vec<(Membership, Control)>,
    logged_in: Arc<Barrier>,
}

fn team(names: &[&str]) -> Members {
    let controls: Vec<Control> = names.iter().map(|_| Control::new().unwrap()).collect();
    let team = Team::new(Some("ultras".into()), names.iter().zip(&controls).map(|(n, c)| (n.to_string(), c.stop_handle())).collect());
    Members { seats: team.memberships().into_iter().zip(controls).collect(), logged_in: Arc::new(Barrier::new(names.len())) }
}

impl Members {
    fn start<T: Send + 'static>(&mut self, body: impl FnOnce(&mut Session) -> T + Send + 'static) -> JoinHandle<T> {
        let (membership, control) = self.seats.remove(0);
        let logged_in = self.logged_in.clone();
        thread::spawn(move || {
            let servers = Servers::start(FARM, |_, _| {});
            let mut session = Session::new(control, servers.credentials(), "5.31", None, Rc::new(|_| {}));
            session.login(Some("Fake")).unwrap();
            session.set_team(Some(membership));
            logged_in.wait();
            let result = body(&mut session);
            session.set_team(None);
            session.logout();
            servers.stop();
            result
        })
    }
}

fn idle(session: &mut Session, for_: Duration) {
    let _ = session.pump_until(Some(Instant::now() + for_), |_| None::<()>);
}

#[test]
fn sync_waits_for_the_last_member_and_returns_every_value() {
    let mut members = team(&["a", "b", "c"]);
    let late = Duration::from_millis(400);
    let threads: Vec<_> = (0..3)
        .map(|index| {
            members.start(move |session| {
                if index == 2 {
                    idle(session, late);
                }
                let started = Instant::now();
                let values = session.sync("ready", json!(index), Some(Instant::now() + PATIENCE));
                (values, started.elapsed())
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    for (index, (values, _)) in results.iter().enumerate() {
        assert_eq!(values.as_ref().ok(), Some(&vec![json!(0), json!(1), json!(2)]), "member {index}");
    }
    assert!(results[0].1 >= late / 2, "the first member waited for the last: {:?}", results[0].1);
    assert!(results[2].1 < late / 2, "the last member didn't wait: {:?}", results[2].1);
}

#[test]
fn a_teammates_share_and_cell_wake_a_waiting_member() {
    let mut members = team(&["a", "b"]);
    let watcher = members.start(|session| {
        let team = session.team().unwrap().team().clone();
        let started = Instant::now();
        let seen = session.pump_until(Some(Instant::now() + PATIENCE), |_| {
            let b = &team.members()[1];
            b.shared.get("go").map(|go| (go.clone(), b.live.cell.clone(), b.live.online))
        });
        (seen.ok(), started.elapsed())
    });
    let mover = members.start(|session| {
        idle(session, Duration::from_millis(300));
        session.jump("r1", "Left").unwrap();
        idle(session, Duration::from_millis(200));
        session.share(json!({ "go": true }).as_object().unwrap().clone());
        idle(session, Duration::from_millis(300));
    });
    mover.join().unwrap();
    let (seen, waited) = watcher.join().unwrap();
    assert_eq!(seen, Some((json!(true), "r1".to_string(), true)));
    assert!(waited < Duration::from_secs(3), "woke on the share, not the deadline: {waited:?}");
}

#[test]
fn a_member_that_gives_up_fails_the_barrier_for_the_rest() {
    let mut members = team(&["a", "b"]);
    let impatient = members.start(|session| session.sync("early", Value::Null, Some(Instant::now() + Duration::from_millis(200))));
    let late = members.start(|session| {
        idle(session, Duration::from_millis(600));
        session.sync("early", Value::Null, Some(Instant::now() + PATIENCE))
    });
    let gave_up = impatient.join().unwrap();
    assert!(matches!(&gave_up, Err(ActError::Timeout(why)) if why == r#"waiting for b at "early""#), "{gave_up:?}");
    let failed = late.join().unwrap();
    assert!(matches!(&failed, Err(ActError::Rejected(why)) if why == r#"a stopped waiting at "early" for b"#), "{failed:?}");
}

#[test]
fn leaving_the_team_releases_a_waiting_teammate_with_an_error() {
    let mut members = team(&["a", "b"]);
    let waiter = members.start(|session| {
        let started = Instant::now();
        (session.sync("loot", Value::Null, Some(Instant::now() + PATIENCE)), started.elapsed())
    });
    let quitter = members.start(|session| idle(session, Duration::from_millis(300)));
    quitter.join().unwrap();
    let (result, waited) = waiter.join().unwrap();
    assert!(matches!(&result, Err(ActError::Rejected(why)) if why == r#"b's script returned before "loot""#), "{result:?}");
    assert!(waited < Duration::from_secs(3), "{waited:?}");
}
