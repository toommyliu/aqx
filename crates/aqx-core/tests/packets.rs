mod server;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aqx_core::Event;
use server::{FARM, Fake};

fn packets_after_a_jump(fake: &mut Fake, seen: &Rc<RefCell<Vec<String>>>, cell: &str) -> Vec<String> {
    seen.borrow_mut().clear();
    fake.session.jump(cell, "Left").unwrap();
    let marker = |_: &aqx_core::Session| seen.borrow().iter().any(|p| p.contains("%marker%")).then_some(());
    fake.session.pump_until(Some(Instant::now() + Duration::from_secs(2)), marker).unwrap();
    seen.borrow().clone()
}

#[test]
fn listening_to_every_packet_hands_each_one_to_the_script_until_it_is_turned_off() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%") {
            wire.send("%xt%uotls%-1%hero%afk:false%");
            wire.send("%xt%marker%-1%");
        }
    });
    let seen = Rc::new(RefCell::new(Vec::new()));
    let collect = seen.clone();
    fake.session.set_hook(Some(Box::new(move |s| {
        for event in s.take_script_events() {
            if let Event::Packet { text } = event {
                collect.borrow_mut().push(text);
            }
        }
    })));
    fake.session.listen("marker", true);

    fake.session.listen_all(true);
    assert!(packets_after_a_jump(&mut fake, &seen, "r2").contains(&"%xt%uotls%-1%hero%afk:false%".to_string()));

    fake.session.listen_all(false);
    let after = packets_after_a_jump(&mut fake, &seen, "r1");
    assert!(!after.iter().any(|p| p.contains("%uotls%")), "{after:?}");
}
