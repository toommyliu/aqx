mod server;

use std::time::{Duration, Instant};

use aqx_core::State;
use server::{FARM, Fake};

#[test]
fn after_respawning_the_bot_goes_back_to_the_cell_it_died_in() {
    let mut killed = false;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%moveToCell%5%r2%") && !killed {
            killed = true;
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intHP":0,"intState":0}}}"#);
        } else if msg.contains("%resPlayerTimed%") {
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intHP":3000,"intState":1}}}"#);
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    let mut died = false;
    fake.session
        .pump_until(Some(Instant::now() + Duration::from_secs(15)), |s| {
            let dead = s.world.me.as_ref().is_some_and(|m| m.vitals.state == State::Dead);
            died |= dead;
            (died && !dead).then_some(())
        })
        .unwrap();
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();
    let respawn = sent.iter().position(|m| m.contains("%resPlayerTimed%")).expect("asked to respawn");
    let moves: Vec<&String> = sent[respawn..].iter().filter(|m| m.contains("%moveToCell%")).collect();
    assert_eq!(moves, ["%xt%zm%moveToCell%5%Enter%Spawn%", "%xt%zm%moveToCell%5%r2%Left%"]);
}
