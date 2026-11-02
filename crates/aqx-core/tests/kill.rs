mod server;

use std::time::{Duration, Instant};

use aqx_core::{Fight, ItemRef};
use server::{FARM, Fake, Wire};

fn killed_on(cast: usize) -> impl FnMut(&str, &mut Wire) + Send {
    let mut casts = 0;
    move |msg, wire| {
        if !msg.contains("%gar%") {
            return;
        }
        casts += 1;
        let target: String = msg.split(">m:").nth(1).unwrap_or("2").chars().take_while(char::is_ascii_digit).collect();
        if casts >= cast {
            wire.object(&format!(r#"{{"cmd":"ct","m":{{"{target}":{{"intHP":0,"intState":0}}}}}}"#));
            wire.object(&format!(r#"{{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":{target}}}"#));
        } else {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":500,"intState":2}},"p":{"hero":{"intState":2}}}"#);
        }
    }
}

fn treeant(m: &aqx_core::Monster) -> bool {
    m.name == "Treeant"
}

#[test]
fn kill_hunts_down_the_target_and_paces_its_skills() {
    let mut fake = Fake::start(FARM, killed_on(3));
    fake.session.kill(&treeant, &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    let sent = fake.finish();

    let jump = sent.iter().position(|(_, m)| m == "%xt%zm%moveToCell%5%r2%Left%").expect("jumped to the Treeant's cell");
    let casts: Vec<&(Instant, String)> = sent.iter().filter(|(_, m)| m.contains("%gar%")).collect();
    assert_eq!(casts.len(), 3);
    assert!(sent.iter().position(|(_, m)| m.contains("%gar%")).unwrap() > jump);
    for (i, (_, msg)) in casts.iter().enumerate() {
        let fields: Vec<&str> = msg.split('%').collect();
        assert_eq!(fields[5], i.to_string(), "action counter in {msg}");
        assert!(fields[6].contains(">m:2") || fields[6].contains(">p:1001"), "target in {msg}");
    }
    let class: Vec<&Instant> = casts.iter().filter(|(_, m)| !m.contains("%aa>")).map(|(t, _)| t).collect();
    assert!(class.len() >= 2 && casts.iter().any(|(_, m)| m.contains("%aa>")), "class skills and an auto attack: {casts:?}");
    assert!(*class[1] - *class[0] >= Duration::from_millis(1450), "class skills {:?} apart", *class[1] - *class[0]);
}

#[test]
fn kill_stops_casting_while_the_target_counters() {
    let mut kill = killed_on(4);
    let mut casts = 0;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") {
            casts += 1;
            if casts == 1 {
                wire.object(
                    r#"{"cmd":"ct","anims":[{"cInf":"m:2","tInf":"p:1001","animStr":"Charge","msg":"Treeant prepares a counter attack!!"}],
                    "a":[{"cmd":"aura++","cInf":"m:2","tInf":"m:2","auras":[{"nam":"Counter Attack","dur":6}]}]}"#,
                );
                wire.mark("<countering>");
                wire.later(
                    Duration::from_millis(3000),
                    "<cleared>",
                    r#"{"cmd":"ct","a":[{"cmd":"aura--","tInf":"m:2","aura":{"nam":"Counter Attack"}}]}"#.into(),
                );
                return;
            }
        }
        kill(msg, wire);
    });
    fake.session.kill(&treeant, &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    let sent = fake.finish();

    let at = |marker: &str| sent.iter().find(|(_, m)| m == marker).map(|(t, _)| *t).unwrap();
    let (countering, cleared) = (at("<countering>"), at("<cleared>"));
    let in_flight = Duration::from_millis(100);
    let during: Vec<&String> =
        sent.iter().filter(|(t, m)| m.contains("%gar%") && *t > countering + in_flight && *t < cleared).map(|(_, m)| m).collect();
    assert!(during.is_empty(), "cast while the target was countering: {during:?}");
}

#[test]
fn hunting_goes_where_a_target_respawns() {
    const DEAD: &str = r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
        "mondef":[{"MonID":12,"strMonName":"Treeant"}],
        "monmap":[{"MonMapID":1,"MonID":12,"strFrame":"r2"},{"MonMapID":2,"MonID":12,"strFrame":"r2"},{"MonMapID":3,"MonID":12,"strFrame":"r3"}],
        "monBranch":[{"MonMapID":1,"intHP":0,"intHPMax":900,"intState":0},{"MonMapID":2,"intHP":0,"intHPMax":900,"intState":0},{"MonMapID":3,"intHP":0,"intHPMax":900,"intState":0}],
        "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}]}"#;
    let mut kill = killed_on(1);
    let mut respawned = false;
    let mut fake = Fake::start(DEAD, move |msg, wire| {
        if msg.contains("%moveToCell%5%r2%") && !respawned {
            respawned = true;
            wire.later(Duration::from_millis(1500), "<later>", r#"{"cmd":"mtls","id":3,"o":{"intHP":900,"intState":1}}"#.into());
        }
        kill(msg, wire);
    });
    fake.session.kill(&treeant, &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    let sent: Vec<String> = fake.finish().into_iter().map(|(_, m)| m).collect();

    let at = |needle: &str| sent.iter().position(|m| m.contains(needle)).unwrap_or_else(|| panic!("never saw {needle}: {sent:#?}"));
    assert!(at("%moveToCell%5%r2%") < at("<later>"), "waited where most Treeants spawn");
    assert!(at("<later>") < at("%moveToCell%5%r3%"), "moved to r3 once its Treeant respawned");
    assert!(at("%moveToCell%5%r3%") < at(">m:3%"), "cast at the respawned Treeant");
}

#[test]
fn farm_picks_up_goal_drops_until_the_count_is_reached() {
    let mut casts = 0;
    let mut held = 0;
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") {
            casts += 1;
            if casts % 2 == 0 {
                wire.object(r#"{"cmd":"dropItem","items":{"333":{"ItemID":333,"sName":"Treeant Branch","iQty":1,"iStk":10}}}"#);
                wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
                wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
                wire.later(Duration::from_millis(300), "<respawn>", r#"{"cmd":"mtls","id":2,"o":{"intHP":900,"intState":1}}"#.into());
            }
        } else if msg.contains("%getDrop%5%333%") {
            held += 1;
            wire.object(&format!(r#"{{"cmd":"getDrop","bSuccess":1,"ItemID":333,"CharItemID":9,"iQty":1,"iQtyNow":{held},"bBank":0}}"#));
        }
    });
    let goals = [(ItemRef::Name("treeant branch".into()), 2)];
    fake.session.farm(&goals, &treeant, &mut Fight::default(), None, Some(Duration::from_secs(20))).unwrap();
    assert_eq!(fake.session.world.count_of(&goals[0].0), 2);
    let sent = fake.finish();
    assert_eq!(sent.iter().filter(|(_, m)| m.contains("%getDrop%")).count(), 2);
}
