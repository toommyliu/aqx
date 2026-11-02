use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use aqx_core::{Control, Fight, Login, Phase, Server, Session};

fn json(o: &str) -> String {
    format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#)
}

const TWO_CELLS: &str = r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
    "mondef":[{"MonID":10,"strMonName":"Mosquito"},{"MonID":12,"strMonName":"Treeant"}],
    "monmap":[{"MonMapID":1,"MonID":10,"strFrame":"r1"},{"MonMapID":2,"MonID":12,"strFrame":"r2"}],
    "monBranch":[{"MonMapID":1,"intHP":540,"intHPMax":540,"intState":1},{"MonMapID":2,"intHP":900,"intHPMax":900,"intState":1}],
    "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}]}"#;

struct Script {
    area: &'static str,
    kill_after: usize,
    counter_after: Option<usize>,
    later: Option<(Duration, &'static str)>,
}

fn play_server(listener: TcpListener, seen: Sender<(Instant, String)>, script: Script) {
    let Script { area, kill_after, counter_after, later } = script;
    let (mut sock, _) = listener.accept().unwrap();
    let send = |sock: &mut TcpStream, msg: &str| {
        sock.write_all(msg.as_bytes()).unwrap();
        sock.write_all(b"\0").unwrap();
    };
    let mut pending = Vec::new();
    let mut casts = 0;
    let mut chunk = [0u8; 4096];
    loop {
        let n = match sock.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        pending.extend_from_slice(&chunk[..n]);
        while let Some(end) = pending.iter().position(|b| *b == 0) {
            let msg = String::from_utf8(pending.drain(..=end).take(end).collect()).unwrap();
            let _ = seen.send((Instant::now(), msg.clone()));
            if msg.contains("verChk") {
                send(&mut sock, "<msg t='sys'><body action='apiOK' r='0'></body></msg>");
            } else if msg.contains("action='login'") {
                send(&mut sock, "%xt%loginResponse%-1%true%1001%hero%Welcome!%");
            } else if msg.contains("%firstJoin%") {
                send(&mut sock, "<msg t='sys'><body action='joinOK' r='5'><uLs r='5'><u i='1001'><n><![CDATA[hero]]></n></u></uLs></body></msg>");
                send(&mut sock, &json(area));
                send(&mut sock, "%xt%server%-1%You joined \"farm-1\"%");
            } else if msg.contains("%retrieveInventory%") {
                send(
                    &mut sock,
                    &json(
                        r#"{"cmd":"sAct","actions":{"active":[
                        {"ref":"aa","nam":"Auto Attack","cd":2000,"mp":-10,"tgt":"h","tgtMax":1},
                        {"ref":"a1","nam":"Slash","cd":3000,"mp":10,"tgt":"h","tgtMax":3},
                        {"ref":"a2","nam":"Guard","cd":3000,"mp":10,"tgt":"s","tgtMax":1},
                        {"ref":"a3","nam":"Cleave","cd":6000,"mp":20,"tgt":"h","tgtMax":1},
                        {"ref":"a4","nam":"Finisher","cd":12000,"mp":30,"tgt":"h","tgtMax":1}]}}"#,
                    ),
                );
                send(&mut sock, &json(r#"{"cmd":"loadInventoryBig","items":[],"factions":[]}"#));
                if let Some((delay, msg)) = later {
                    let mut later_sock = sock.try_clone().unwrap();
                    let seen = seen.clone();
                    thread::spawn(move || {
                        thread::sleep(delay);
                        let _ = seen.send((Instant::now(), "<later>".into()));
                        send(&mut later_sock, &json(msg));
                    });
                }
            } else if msg.contains("%gar%") {
                casts += 1;
                let target: String = msg.split(">m:").nth(1).unwrap_or("2").chars().take_while(char::is_ascii_digit).collect();
                if counter_after == Some(casts) {
                    send(
                        &mut sock,
                        &json(
                            r#"{"cmd":"ct","anims":[{"cInf":"m:2","tInf":"p:1001","animStr":"Charge","msg":"Treeant prepares a counter attack!!"}],
                            "a":[{"cmd":"aura++","cInf":"m:2","tInf":"m:2","auras":[{"nam":"Counter Attack","dur":6}]}]}"#,
                        ),
                    );
                    let _ = seen.send((Instant::now(), "<countering>".into()));
                    let mut later = sock.try_clone().unwrap();
                    let seen = seen.clone();
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(3000));
                        let _ = seen.send((Instant::now(), "<cleared>".into()));
                        send(&mut later, &json(r#"{"cmd":"ct","a":[{"cmd":"aura--","tInf":"m:2","aura":{"nam":"Counter Attack"}}]}"#));
                    });
                } else if casts >= kill_after {
                    send(&mut sock, &json(&format!(r#"{{"cmd":"ct","m":{{"{target}":{{"intHP":0,"intState":0}}}}}}"#)));
                    send(&mut sock, &json(&format!(r#"{{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":{target}}}"#)));
                } else {
                    send(&mut sock, &json(r#"{"cmd":"ct","m":{"2":{"intHP":500,"intState":2}},"p":{"hero":{"intState":2}}}"#));
                }
            } else if msg.contains("%logout%") {
                send(&mut sock, "<msg t='sys'><body action='logout' r='0'></body></msg>");
                return;
            }
        }
    }
}

#[test]
fn kill_hunts_down_the_target_and_paces_its_skills() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let script = Script { area: TWO_CELLS, kill_after: 3, counter_after: None, later: None };
    let server_thread = thread::spawn(move || play_server(listener, tx, script));
    let server = Server {
        name: "Fake".into(),
        host: "127.0.0.1".into(),
        port,
        online: true,
        members_only: false,
        players: 0,
        capacity: 0,
    };
    let login = Login { user: "hero", token: "t", version: "5.31" };
    let mut session = Session::connect(Control::new().unwrap(), &server, login, None, |_| {}).unwrap();
    session.pump_until(Some(Instant::now() + Duration::from_secs(5)), |s| (s.phase == Phase::Ready).then_some(())).unwrap();

    session.kill(&|m| m.name == "Treeant", &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    session.logout();
    server_thread.join().unwrap();

    let sent: Vec<(Instant, String)> = rx.try_iter().collect();
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let script = Script { area: TWO_CELLS, kill_after: 4, counter_after: Some(1), later: None };
    let server_thread = thread::spawn(move || play_server(listener, tx, script));
    let server = Server { name: "Fake".into(), host: "127.0.0.1".into(), port, online: true, members_only: false, players: 0, capacity: 0 };
    let login = Login { user: "hero", token: "t", version: "5.31" };
    let mut session = Session::connect(Control::new().unwrap(), &server, login, None, |_| {}).unwrap();
    session.pump_until(Some(Instant::now() + Duration::from_secs(5)), |s| (s.phase == Phase::Ready).then_some(())).unwrap();

    session.kill(&|m| m.name == "Treeant", &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    session.logout();
    server_thread.join().unwrap();

    let sent: Vec<(Instant, String)> = rx.try_iter().collect();
    let at = |marker: &str| sent.iter().find(|(_, m)| m == marker).map(|(t, _)| *t).unwrap();
    let (countering, cleared) = (at("<countering>"), at("<cleared>"));
    let in_flight = Duration::from_millis(100);
    let during: Vec<&String> =
        sent.iter().filter(|(t, m)| m.contains("%gar%") && *t > countering + in_flight && *t < cleared).map(|(_, m)| m).collect();
    assert!(during.is_empty(), "cast while the target was countering: {during:?}");
    assert_eq!(sent.iter().filter(|(_, m)| m.contains("%gar%")).count(), 4);
}

#[test]
fn hunting_goes_where_a_target_respawns() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let script = Script {
        area: r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
            "mondef":[{"MonID":12,"strMonName":"Treeant"}],
            "monmap":[{"MonMapID":1,"MonID":12,"strFrame":"r2"},{"MonMapID":2,"MonID":12,"strFrame":"r2"},{"MonMapID":3,"MonID":12,"strFrame":"r3"}],
            "monBranch":[{"MonMapID":1,"intHP":0,"intHPMax":900,"intState":0},{"MonMapID":2,"intHP":0,"intHPMax":900,"intState":0},{"MonMapID":3,"intHP":0,"intHPMax":900,"intState":0}],
            "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}]}"#,
        kill_after: 1,
        counter_after: None,
        later: Some((Duration::from_millis(1500), r#"{"cmd":"mtls","id":3,"o":{"intHP":900,"intState":1}}"#)),
    };
    let server_thread = thread::spawn(move || play_server(listener, tx, script));
    let server = Server { name: "Fake".into(), host: "127.0.0.1".into(), port, online: true, members_only: false, players: 0, capacity: 0 };
    let login = Login { user: "hero", token: "t", version: "5.31" };
    let mut session = Session::connect(Control::new().unwrap(), &server, login, None, |_| {}).unwrap();
    session.pump_until(Some(Instant::now() + Duration::from_secs(5)), |s| (s.phase == Phase::Ready).then_some(())).unwrap();

    session.kill(&|m| m.name == "Treeant", &mut Fight::default(), None, Some(Duration::from_secs(10))).unwrap();
    session.logout();
    server_thread.join().unwrap();

    let sent: Vec<String> = rx.try_iter().map(|(_, m)| m).collect();
    let at = |needle: &str| sent.iter().position(|m| m.contains(needle)).unwrap_or_else(|| panic!("never saw {needle}: {sent:#?}"));
    assert!(at("%moveToCell%5%r2%") < at("<later>"), "waited where most Treeants spawn");
    assert!(at("<later>") < at("%moveToCell%5%r3%"), "moved to r3 once its Treeant respawned");
    assert!(at("%moveToCell%5%r3%") < at(">m:3%"), "cast at the respawned Treeant");
}
