mod server;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use aqx_core::{ActError, Disconnect, Fight, Halt, Item, ItemRef, Monster, Phase, State, World};
use aqx_protocol::{ItemId, MonMapId};
use server::{FARM, Fake};

fn sent(fake: Fake) -> Vec<(Instant, String)> {
    fake.finish()
}

fn messages(sent: &[(Instant, String)]) -> Vec<&str> {
    sent.iter().map(|(_, m)| m.as_str()).collect()
}

fn pump_for(fake: &mut Fake, wait: Duration) {
    let _ = fake.session.pump_until(Some(Instant::now() + wait), |_| None::<()>);
}

#[test]
fn being_marked_afk_is_answered_like_the_client_does() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%5%r1%") {
            wire.send("%xt%uotls%-1%hero%afk:true%");
        } else if msg == "%xt%zm%afk%1%false%" {
            wire.send("%xt%uotls%-1%hero%afk:false%");
        }
    });
    fake.session.jump("r1", "Left").unwrap();
    pump_for(&mut fake, Duration::from_millis(300));
    assert!(!fake.session.world.me.as_ref().unwrap().afk);
    let sent = sent(fake);
    assert_eq!(messages(&sent).iter().filter(|m| **m == "%xt%zm%afk%1%false%").count(), 1);
}

#[test]
fn aggro_pulls_live_matches_in_the_cell_every_half_second() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.jump("r2", "Left").unwrap();
    fake.session.set_aggro(Some(Box::new(|_: &World, m: &Monster| m.name == "Treeant")));
    pump_for(&mut fake, Duration::from_millis(1200));
    fake.session.set_aggro(None);
    pump_for(&mut fake, Duration::from_millis(600));
    let sent = sent(fake);
    let pulls: Vec<&(Instant, String)> = sent.iter().filter(|(_, m)| m.contains("%aggroMon%")).collect();
    assert!((2..=4).contains(&pulls.len()), "{pulls:?}");
    assert!(pulls.iter().all(|(_, m)| m == "%xt%zm%aggroMon%5%2%"));
    assert!(pulls[1].0 - pulls[0].0 >= Duration::from_millis(450));
}

#[test]
fn a_dropped_connection_closes_the_session_and_a_relogin_restores_it() {
    let mut dropped = false;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%moveToCell%5%r1%") && !dropped {
            dropped = true;
            wire.hang_up();
        }
    });
    fake.session.world.me.as_mut().unwrap().gold = 5;
    let lost = fake.session.jump("r1", "Left").and_then(|()| Ok(fake.session.pump_until(Some(Instant::now() + Duration::from_secs(2)), |_| None::<()>)?));
    let Err(ActError::Halted(Halt::Closed(why))) = lost else { panic!("expected the connection to close, got {lost:?}") };
    assert!(matches!(why, Disconnect::ConnectionLost(_)) && why.retryable(), "{why:?}");

    fake.session.wait_to_relogin(why.to_string(), Instant::now() + Duration::from_millis(100)).unwrap();
    fake.session.login(Some("Fake")).unwrap();
    assert_eq!(fake.session.phase, Phase::Ready);
    assert_eq!(fake.session.world.me.as_ref().unwrap().gold, 0, "the world starts over");
    fake.session.jump("r2", "Left").unwrap();
    assert_eq!(fake.logins.load(Ordering::SeqCst), 2);
    let sent = sent(fake);
    assert_eq!(messages(&sent).iter().filter(|m| m.contains("action='login'")).count(), 2);
}

#[test]
fn relogin_logs_out_first_and_picks_the_server_again() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.relogin(None).unwrap();
    assert_eq!(fake.session.server().map(|s| s.name.as_str()), Some("Fake"));
    let servers = fake.session.servers().unwrap();
    assert_eq!(servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Fake"]);
    let sent = sent(fake);
    let all = messages(&sent);
    let logout = all.iter().position(|m| m.contains("%logout%")).unwrap();
    assert!(all[logout..].iter().any(|m| m.contains("action='login'")));
}

#[test]
fn exit_jumps_to_the_same_cell_until_combat_ends() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%gar%") {
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intState":2}},"m":{"2":{"intHP":800,"intState":2}}}"#);
        } else if msg == "%xt%zm%moveToCell%5%r2%Left%" {
            wire.object(r#"{"cmd":"uotls","unm":"hero","o":{"intState":1}}"#);
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    fake.session.use_skill(0, None).unwrap();
    pump_for(&mut fake, Duration::from_millis(200));
    assert_eq!(fake.session.world.me.as_ref().unwrap().vitals.state, State::Combat);
    fake.session.exit_combat().unwrap();
    assert_eq!(fake.session.world.me.as_ref().unwrap().vitals.state, State::Idle);
    let sent = sent(fake);
    assert_eq!(messages(&sent).iter().filter(|m| **m == "%xt%zm%moveToCell%5%r2%Left%").count(), 2);
}

fn potion() -> Item {
    Item {
        id: ItemId(11857),
        char_item: None,
        name: "Health Potion".into(),
        qty: 3,
        max_stack: 10,
        kind: "Item".into(),
        slot: "None".into(),
        equipped: true,
        ac: false,
        member: false,
        house: false,
        link: "potion".into(),
        file: String::new(),
        icon: String::new(),
        meta: "457".into(),
        level: 0,
        enhancement: None,
    }
}

#[test]
fn skills_use_casts_a_slot_now_and_slot_five_drinks_the_equipped_potion() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let none = fake.session.use_skill(1, None);
    assert!(matches!(none, Err(ActError::NotFound(_))), "nothing to hit in Enter: {none:?}");
    fake.session.world.inventory.push(potion());
    fake.session.world.me.as_mut().unwrap().item_skill = Some(457);
    fake.session.use_skill(5, None).unwrap();
    fake.session.jump("r1", "Left").unwrap();
    let mosquito = |_: &World, m: &Monster| m.name == "Mosquito";
    fake.session.use_skill(1, Some(&mosquito)).unwrap();
    let sent = sent(fake);
    let casts: Vec<&str> = messages(&sent).into_iter().filter(|m| m.contains("%gar%")).collect();
    assert_eq!(casts, ["%xt%zm%gar%1%0%i1>p:1001%11857%wvz%", "%xt%zm%gar%1%1%a1>m:1%wvz%"]);
}

#[test]
fn collect_repeats_get_map_item_until_the_count_is_reached() {
    let mut held = 0;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg == "%xt%zm%getMapItem%5%923%" {
            held += 1;
            wire.object(&format!(r#"{{"cmd":"addItems","items":{{"77":{{"ItemID":77,"sName":"Ore","bTemp":true,"iQty":1,"iQtyNow":{held}}}}}}}"#));
        }
    });
    let ore = ItemRef::Name("Ore".into());
    fake.session.collect(923, &ore, 2).unwrap();
    assert_eq!(fake.session.world.count_of(&ore), 2);
    let missing = fake.session.collect(924, &ore, 3);
    assert!(matches!(&missing, Err(ActError::Rejected(why)) if why.contains("map item 924")), "{missing:?}");
    let sent = sent(fake);
    let gets: Vec<&(Instant, String)> = sent.iter().filter(|(_, m)| m == "%xt%zm%getMapItem%5%923%").collect();
    assert_eq!(gets.len(), 2);
    assert!(gets[1].0 - gets[0].0 >= Duration::from_millis(950), "getMapItem keeps the client's 1 s lock");
}

#[test]
fn raw_packets_go_through_the_pacer() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.send_raw("%xt%zm%cmd%5%tfer%hero%farm%").unwrap();
    fake.session.send_raw("%xt%zm%cmd%5%tfer%hero%farm%").unwrap();
    assert!(matches!(fake.session.send_raw("hello"), Err(ActError::Rejected(_))));
    let sent = sent(fake);
    let raw: Vec<&Instant> = sent.iter().filter(|(_, m)| m == "%xt%zm%cmd%5%tfer%hero%farm%").map(|(t, _)| t).collect();
    assert_eq!(raw.len(), 2);
    assert!(*raw[1] - *raw[0] >= Duration::from_millis(2950), "tfer keeps its 3 s lock: {:?}", *raw[1] - *raw[0]);
}

#[test]
fn pause_holds_actions_and_the_fight_until_resume() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%gar%") {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":500,"intState":2}}}"#);
        }
    });
    let handle = fake.session.stop_handle();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        handle.post(|s| s.set_paused(true));
        std::thread::sleep(Duration::from_millis(1500));
        handle.post(|s| s.set_paused(false));
    });
    let treeant = |_: &World, m: &Monster| m.name == "Treeant";
    let started = Instant::now();
    let result = fake.session.kill(&treeant, &mut Fight::default(), None, Some(Duration::from_millis(2500)));
    assert!(matches!(result, Err(ActError::Timeout(_))), "{result:?}");
    let sent = sent(fake);
    let quiet = sent.iter().filter(|(t, m)| m.contains("%gar%") && *t > started + Duration::from_millis(400) && *t < started + Duration::from_millis(1750));
    assert_eq!(quiet.count(), 0, "cast while paused");
    assert!(sent.iter().any(|(t, m)| m.contains("%gar%") && *t > started + Duration::from_millis(1800)), "resumed casting");
}

#[test]
fn a_failed_login_closes_the_session() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let refused = fake.session.relogin(Some("Nowhere"));
    assert!(matches!(&refused, Err(ActError::Rejected(why)) if why.contains("no server named Nowhere")), "{refused:?}");
    assert!(matches!(fake.session.phase, Phase::Closed(_)), "{:?}", fake.session.phase);
    let after = fake.session.jump("r1", "Left");
    assert!(matches!(after, Err(ActError::Halted(Halt::Closed(_)))), "{after:?}");
    fake.finish();
}

#[test]
fn skills_use_waits_for_mana() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg == "%xt%zm%moveToCell%5%r1%Left%" {
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intMP":0}}}"#);
            wire.later(Duration::from_millis(900), "<mana>", r#"{"cmd":"ct","p":{"hero":{"intMP":100}}}"#.into());
        }
    });
    fake.session.jump("r1", "Left").unwrap();
    let started = Instant::now();
    fake.session.use_skill(1, None).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(250), "cast after {:?}", started.elapsed());
    let sent = sent(fake);
    let all = messages(&sent);
    let mana = all.iter().position(|m| *m == "<mana>").expect("mana came back");
    assert_eq!(all.iter().filter(|m| m.contains("%gar%")).count(), 1);
    assert!(all.iter().position(|m| m.contains("%gar%")).unwrap() > mana, "cast before the mana came back");
}

fn attacks(sent: &[(Instant, String)]) -> Vec<&(Instant, String)> {
    sent.iter().filter(|(_, m)| m.contains("%gar%")).collect()
}

#[test]
fn a_target_set_from_outside_is_auto_attacked_until_it_dies() {
    let mut hits = 0;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") {
            hits += 1;
            if hits == 2 {
                wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
            }
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    fake.session.set_target(Some(MonMapId(2)));
    let cleared = fake.session.pump_until(Some(Instant::now() + Duration::from_secs(5)), |s| s.world.target.is_none().then(Instant::now));
    let sent = sent(fake);

    let attacks = attacks(&sent);
    assert_eq!(attacks.len(), 2, "{attacks:?}");
    assert!(attacks.iter().all(|(_, m)| m.contains("%aa>m:2%")), "{attacks:?}");
    assert!(attacks[1].0 - attacks[0].0 >= Duration::from_millis(2000), "auto attacks {:?} apart", attacks[1].0 - attacks[0].0);
    let after = cleared.expect("the dead target was cleared") - attacks[1].0;
    assert!(after < Duration::from_millis(400), "cleared {after:?} after the killing blow");
}

#[test]
fn cast_now_sends_a_ready_skill_at_the_target_and_refuses_one_on_cooldown() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.jump("r2", "Left").unwrap();
    assert!(!fake.session.cast_now(1), "cast a hostile skill with no target");
    fake.session.set_target(Some(MonMapId(2)));
    assert!(fake.session.can_cast(1));
    assert!(fake.session.cast_now(1));
    assert!(!fake.session.can_cast(3) && !fake.session.cast_now(3), "cast inside the global cooldown");
    pump_for(&mut fake, Duration::from_millis(1600));
    assert!(fake.session.cast_now(3));
    let sent = sent(fake);

    let class: Vec<&str> = attacks(&sent).into_iter().map(|(_, m)| m.as_str()).filter(|m| !m.contains("%aa>")).collect();
    assert_eq!(class.len(), 2, "{class:?}");
    assert!(class[0].contains("%a1>m:2%") && class[1].contains("%a3>m:2%"), "{class:?}");
}

#[test]
fn the_end_of_a_script_stops_the_background_attack() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.jump("r2", "Left").unwrap();
    fake.session.set_target(Some(MonMapId(2)));
    pump_for(&mut fake, Duration::from_millis(300));
    fake.session.set_hook(None);
    pump_for(&mut fake, Duration::from_millis(2500));
    assert_eq!(fake.session.world.target, None);
    let sent = sent(fake);
    assert_eq!(attacks(&sent).len(), 1, "{:?}", attacks(&sent));
}

#[test]
fn can_cast_refuses_a_skill_the_class_rank_has_not_unlocked() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%5%r2%") {
            wire.object(
                r#"{"cmd":"sAct","actions":{"active":[
                {"ref":"aa","nam":"Auto Attack","cd":2000,"mp":0,"tgt":"h","tgtMax":1,"isOK":true},
                {"ref":"a1","nam":"Footwork","cd":3000,"mp":10,"tgt":"h","tgtMax":1,"isOK":false}]}}"#,
            );
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    pump_for(&mut fake, Duration::from_millis(200));
    fake.session.set_target(Some(MonMapId(2)));

    assert!(!fake.session.can_cast(1));
    assert!(!fake.session.cast_now(1));
    let used = fake.session.use_skill(1, None);
    assert!(matches!(&used, Err(ActError::Rejected(why)) if why.contains("Footwork is locked")), "{used:?}");
    let sent = sent(fake);
    assert!(!sent.iter().any(|(_, m)| m.contains("%a1>")), "{:?}", messages(&sent));
}

#[test]
fn skills_the_class_rank_has_not_unlocked_are_never_cast() {
    let mut casts = 0;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%moveToCell%5%r2%") {
            wire.object(
                r#"{"cmd":"sAct","actions":{"active":[
                {"ref":"aa","nam":"Auto Attack","cd":2000,"mp":0,"tgt":"h","tgtMax":1,"isOK":true},
                {"ref":"a1","nam":"Viper's Kiss","cd":3000,"mp":0,"tgt":"h","tgtMax":1,"isOK":true},
                {"ref":"a2","nam":"Footwork","cd":3000,"mp":0,"tgt":"h","tgtMax":1,"isOK":false}]}}"#,
            );
        } else if msg.contains("%gar%") {
            casts += 1;
            if casts >= 4 {
                wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
            } else {
                wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":500,"intState":2}},"p":{"hero":{"intState":2}}}"#);
            }
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    pump_for(&mut fake, Duration::from_millis(200));

    let used = fake.session.use_skill(2, None);
    assert!(matches!(&used, Err(ActError::Rejected(why)) if why.contains("Footwork is locked")), "{used:?}");
    let treeant = |_: &World, m: &Monster| m.name == "Treeant";
    fake.session.kill(&treeant, &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    let sent = sent(fake);
    assert!(sent.iter().any(|(_, m)| m.contains("%a1>")), "{:?}", messages(&sent));
    assert!(!sent.iter().any(|(_, m)| m.contains("%a2>")), "{:?}", messages(&sent));
}

#[test]
fn a_player_who_enters_after_us_is_asked_for_their_level() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%5%r2%") {
            wire.send("<msg t='sys'><body action='uER' r='5'><u i='1003' m='0' s='0' p='2'><n><![CDATA[newbie]]></n></u></body></msg>");
        } else if msg.contains("%retrieveUserData%5%1003%") {
            wire.object(r#"{"cmd":"initUserData","uid":1003,"data":{"intLevel":42}}"#);
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    pump_for(&mut fake, Duration::from_millis(300));
    let level = fake.session.world.area.as_ref().and_then(|a| a.players.iter().find(|p| p.name == "newbie")).map(|p| p.level);
    assert_eq!(level, Some(42));
}

const TOWN: &str = r#"{"cmd":"moveToArea","areaId":6,"areaName":"town-2","strMapName":"town","mondef":[],"monmap":[],"monBranch":[],
    "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1},
                {"entID":1002,"uoName":"ally","strFrame":"Enter","strPad":"Spawn","intHP":900,"intHPMax":1000,"intState":1}]}"#;

#[test]
fn going_to_a_player_waits_out_combat_since_the_server_ignores_it_during_and_just_after_a_fight() {
    let mut calm_since: Option<Instant> = None;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") {
            calm_since = None;
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intState":2}},"m":{"2":{"intHP":800,"intState":2}}}"#);
        } else if msg.contains("%moveToCell%") {
            calm_since = Some(Instant::now());
            wire.object(r#"{"cmd":"uotls","unm":"hero","o":{"intState":1}}"#);
        } else if msg.contains("%goto%ally%") && calm_since.is_some_and(|at| at.elapsed() >= Duration::from_millis(1200)) {
            wire.send("<msg t='sys'><body action='joinOK' r='6'><uLs r='6'><u i='1001'><n><![CDATA[hero]]></n></u></uLs></body></msg>");
            wire.object(TOWN);
            wire.send("%xt%server%-1%You joined \"town-2\"%");
        }
    });
    fake.session.jump("r2", "Left").unwrap();
    fake.session.use_skill(0, None).unwrap();
    pump_for(&mut fake, Duration::from_millis(200));
    assert_eq!(fake.session.world.me.as_ref().unwrap().vitals.state, State::Combat);
    fake.session.goto("ally").unwrap();
    assert_eq!(fake.session.world.area.as_ref().map(|a| a.name.as_str()), Some("town-2"));
}

#[test]
fn a_goto_dropped_by_a_monster_pulling_the_player_back_into_combat_is_sent_again_at_once() {
    let mut calm_since: Option<Instant> = None;
    let mut gotos = 0;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") {
            calm_since = None;
            wire.object(r#"{"cmd":"ct","p":{"hero":{"intState":2}},"m":{"2":{"intHP":800,"intState":2}}}"#);
        } else if msg.contains("%moveToCell%") {
            calm_since = Some(Instant::now());
            wire.object(r#"{"cmd":"uotls","unm":"hero","o":{"intState":1}}"#);
        } else if msg.contains("%goto%ally%") {
            gotos += 1;
            if gotos == 1 {
                calm_since = None;
                wire.object(r#"{"cmd":"ct","p":{"hero":{"intState":2}},"m":{"2":{"intHP":800,"intState":2}}}"#);
            } else if calm_since.is_some_and(|at| at.elapsed() >= Duration::from_millis(1200)) {
                wire.send("<msg t='sys'><body action='joinOK' r='6'><uLs r='6'><u i='1001'><n><![CDATA[hero]]></n></u></uLs></body></msg>");
                wire.object(TOWN);
                wire.send("%xt%server%-1%You joined \"town-2\"%");
            }
        }
    });
    let started = Instant::now();
    fake.session.goto("ally").unwrap();
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
    assert_eq!(fake.session.world.area.as_ref().map(|a| a.name.as_str()), Some("town-2"));
}
