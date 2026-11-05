use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde_json::json;

use super::*;
use crate::quest::{Quest, QuestItem, QuestReward, QuestStatus, RewardKind};

const TOKEN: &str = "tok-abc123";
const API_OK: &str = "<msg t='sys'><body action='apiOK' r='0'></body></msg>";
const JOIN_OK: &str = "<msg t='sys'><body action='joinOK' r='0'><uLs r='42'></uLs></body></msg>";
const LOGIN_OK: &str = "%xt%loginResponse%-1%true%7%player1%Welcome%0%sVersion=R0048%5.31%";
const OUTDATED: &str = "%xt%loginResponse%-1%0%-1%%Your game client is an older version. Server requires AQW game version 5.40 or higher. Please close the AQW game window and try again.%";

struct Peer {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Peer {
    fn accept(listener: &TcpListener) -> Peer {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Peer {
            stream,
            buf: Vec::new(),
        }
    }

    fn recv(&mut self) -> String {
        loop {
            if let Some(end) = self.buf.iter().position(|&b| b == 0) {
                let frame: Vec<u8> = self.buf.drain(..=end).collect();
                return String::from_utf8(frame[..end].to_vec()).unwrap();
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).unwrap();
            assert!(n > 0, "the client closed the connection");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn expect(&mut self, packet: &str) {
        assert_eq!(self.recv(), packet);
    }

    fn send(&mut self, text: &str) {
        self.send_bytes(text.as_bytes());
    }

    fn send_bytes(&mut self, frame: &[u8]) {
        self.stream.write_all(frame).unwrap();
        self.stream.write_all(&[0]).unwrap();
    }

    fn expect_logout(&mut self) {
        while self.recv() != packet::logout() {}
    }

    fn wait_for_close(&mut self) {
        let mut chunk = [0u8; 4096];
        while self.stream.read(&mut chunk).unwrap() > 0 {}
    }

    fn serve_to_ready(&mut self, version: &str) {
        self.serve_with_items(
            version,
            json!([
                {"ItemID": 100, "CharItemID": 5000, "sName": "Default Sword", "iQty": 1, "iStk": 1,
                 "sType": "Sword", "sES": "Weapon", "bEquip": 1},
                {"ItemID": "101", "CharItemID": "5001", "sName": "Health Potion", "iQty": "5",
                 "iStk": 10, "sType": "Item", "sES": "None", "bEquip": false},
            ]),
        );
    }

    fn serve_with_items(&mut self, version: &str, items: Value) {
        self.expect(&packet::version_check());
        self.send(API_OK);
        self.expect(&packet::login("player1", version, TOKEN));
        self.send(LOGIN_OK);
        self.expect(&packet::first_join());
        self.expect(&packet::clear_ignore_list());
        self.send(&move_to_area(42, "battleon-1"));
        self.send(JOIN_OK);
        self.send("%xt%server%-1%You joined \"battleon-1\"!%");
        self.expect(&packet::move_to_cell(AreaId(42), "Enter", "Spawn"));
        self.expect(&packet::retrieve_user_data(AreaId(42), EntityId(7)));
        self.expect(&packet::retrieve_user_datas(AreaId(42), &[EntityId(8)]));
        self.send(&json_message(json!({
            "cmd": "initUserData",
            "uid": 7,
            "data": {
                "intLevel": "50", "intGold": 1200, "intCoins": 30, "intExp": 400, "intExpToLevel": 2000,
                "strQuests": format!("00A{}", "0".repeat(97)), "strQuests2": "1".repeat(60),
                "iUpgDays": -1, "iCP": 899,
                "ia0": 5, "iw0": "8", "im0": 2.147483648E9,
                "strEmail": "player1@example.com", "strIP": "192.0.2.1",
            },
        })));
        self.expect(&packet::retrieve_inventory(AreaId(42), EntityId(7)));
        self.send(&json_message(json!({"cmd": "sAct", "actions": {"active": [
            {"ref": "aa", "nam": "Auto Attack", "cd": 2000, "mp": -10, "tgt": "h", "tgtMax": 1},
        ]}})));
        self.send(&json_message(json!({"cmd": "stu", "sta": {"$tha": 0.05}})));
        let inventory = json_message(
            json!({"cmd": "loadInventoryBig", "items": items, "factions": [
                {"FactionID": "46", "CharFactionID": "900", "sName": "Good", "iRep": "1000"},
            ]}),
        );
        self.send_bytes(&compress(&inventory));
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Write);
        let mut chunk = [0u8; 4096];
        while matches!(self.stream.read(&mut chunk), Ok(n) if n > 0) {}
    }
}

fn json_message(o: Value) -> String {
    json!({"t": "xt", "b": {"r": -1, "o": o}}).to_string()
}

fn move_to_area(area: i64, name: &str) -> String {
    json_message(json!({
        "cmd": "moveToArea",
        "areaId": area,
        "areaName": name,
        "uoBranch": [
            {"entID": 7, "strUsername": "player1", "strFrame": "Enter", "intHP": 950, "intHPMax": 1000},
            {"entID": 8, "strUsername": "player2", "strFrame": "Enter"},
            {"entID": 9, "strUsername": "player3", "strFrame": "r2"},
        ],
    }))
}

fn compress(text: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap());
    [b"Z".as_slice(), encoded.as_bytes()].concat()
}

struct Harness {
    session: Handle,
    events: mpsc::Receiver<Event>,
    script: JoinHandle<()>,
}

impl Harness {
    fn start(options: Options, script: impl FnOnce(TcpListener) + Send + 'static) -> Harness {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = Server {
            name: "Test".into(),
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            players: 0,
            capacity: 10,
            online: true,
            members_only: false,
        };
        let account = Account {
            username: "player1".into(),
            token: crate::api::Token::for_tests(TOKEN),
            member: false,
            servers: vec![server.clone()],
        };
        let script = thread::spawn(move || script(listener));
        let (session, events) = spawn(&account, server, options).unwrap();
        Harness {
            session,
            events,
            script,
        }
    }

    fn finish(self) -> Vec<Event> {
        let events = self.events.iter().collect();
        self.script.join().expect("the fake server failed");
        events
    }
}

fn phases(events: &[Event]) -> Vec<Phase> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Phase(phase) => Some(phase.clone()),
            _ => None,
        })
        .collect()
}

fn closed_with(events: &[Event]) -> Option<Disconnect> {
    match phases(events).pop() {
        Some(Phase::Closed { reason }) => Some(reason),
        _ => None,
    }
}

#[test]
fn logs_in_reports_each_phase_and_logs_out() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::logout());
    });
    harness.session.ready().wait().unwrap();
    let me = harness.session.snapshot().me.clone().unwrap();
    assert_eq!(
        (me.level, me.hp, me.max_hp, me.haste, me.skills.len()),
        (50, 950, 1000, 0.05, 1)
    );
    harness.session.logout();

    let events = harness.finish();
    assert_eq!(
        phases(&events),
        [
            Phase::Connecting {
                server: "Test".into(),
                version: "5.31".into()
            },
            Phase::Handshaking,
            Phase::LoggingIn,
            Phase::Loading {
                step: Load::Joining
            },
            Phase::Loading {
                step: Load::Character
            },
            Phase::Loading {
                step: Load::Inventory
            },
            Phase::Ready,
            Phase::Closed {
                reason: Disconnect::LoggedOut
            },
        ]
    );
    assert!(events.contains(&Event::Joined {
        map: "battleon-1".into(),
        cell: "Enter".into(),
        pad: "Spawn".into(),
        players: 2,
    }));
    assert!(events.contains(&Event::Ready {
        level: 50,
        gold: 1200,
        items: 2
    }));
}

#[test]
fn retries_with_the_version_the_server_requires() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut outdated = Peer::accept(&listener);
        outdated.expect(&packet::version_check());
        outdated.send(API_OK);
        outdated.expect(&packet::login("player1", "5.31", TOKEN));
        outdated.send(OUTDATED);
        outdated.wait_for_close();
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.40");
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    harness.session.logout();

    let versions: Vec<String> = phases(&harness.finish())
        .into_iter()
        .filter_map(|p| match p {
            Phase::Connecting { version, .. } => Some(version),
            _ => None,
        })
        .collect();
    assert_eq!(versions, ["5.31", "5.40"]);
}

#[test]
fn a_retry_that_cannot_reconnect_ends_the_login() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut outdated = Peer::accept(&listener);
        drop(listener);
        outdated.expect(&packet::version_check());
        outdated.send(API_OK);
        outdated.expect(&packet::login("player1", "5.31", TOKEN));
        outdated.send(OUTDATED);
        outdated.wait_for_close();
    });
    let started = Instant::now();
    let ready = harness.session.ready().wait();
    assert!(
        matches!(
            ready,
            Err(ActionError::Closed(Disconnect::ConnectionLost(_)))
        ),
        "{ready:?}"
    );
    assert!(started.elapsed() < READY_TIMEOUT / 2);
    harness.finish();
}

#[test]
fn a_pinned_version_is_not_retried() {
    let options = Options {
        client_version: Some("5.20".into()),
        ..Options::default()
    };
    let harness = Harness::start(options, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.expect(&packet::version_check());
        peer.send(API_OK);
        peer.expect(&packet::login("player1", "5.20", TOKEN));
        peer.send(OUTDATED);
        peer.wait_for_close();
    });
    let ready = harness.session.ready().wait();
    assert!(
        matches!(&ready, Err(ActionError::Closed(Disconnect::ClientVersion(v))) if v == "5.40"),
        "{ready:?}"
    );
    harness.finish();
}

#[test]
fn a_rejected_login_reports_the_servers_reason() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.expect(&packet::version_check());
        peer.send(API_OK);
        peer.recv();
        peer.send("%xt%loginResponse%-1%0%-1%%Server is full.%");
        peer.wait_for_close();
    });
    let ready = harness.session.ready().wait();
    assert!(
        matches!(&ready, Err(ActionError::Closed(Disconnect::Rejected(m))) if m == "Server is full."),
        "{ready:?}"
    );
    harness.finish();
}

#[test]
fn later_joins_skip_own_data_and_chat_flows_until_logged_in_elsewhere() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::retrieve_user_datas(AreaId(43), &[EntityId(8)]));
        peer.send("%xt%chatm%-1%zone~hello there%player2%");
        peer.send("%xt%multiLoginWarning%-1%");
    });

    let events = harness.finish();
    assert_eq!(closed_with(&events), Some(Disconnect::LoggedInElsewhere));
    assert!(events.contains(&Event::Chat {
        channel: "zone".into(),
        sender: "player2".into(),
        text: "hello there".into(),
    }));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Joined { map, .. } if map == "yulgar-2"))
    );
}

#[test]
fn joins_a_map_then_moves_between_its_cells() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "yulgar"));
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::retrieve_user_datas(AreaId(43), &[EntityId(8)]));
        peer.expect(&packet::move_to_cell(AreaId(43), "r2", "Left"));
        peer.expect(&packet::retrieve_user_datas(AreaId(43), &[EntityId(9)]));
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::move_to_cell(AreaId(43), "r2", "Left"));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.join("yulgar").wait().unwrap();
    assert_eq!(session.snapshot().room.as_ref().unwrap().name, "yulgar-2");
    session.join("yulgar").wait().unwrap();
    session.move_to_cell("r2", "Left").wait().unwrap();
    session.move_to_cell("Enter", "Spawn").wait().unwrap();
    session.move_to_cell("r2", "Left").wait().unwrap();
    let world = session.snapshot();
    let room = world.room.as_ref().unwrap();
    assert_eq!((room.cell.as_str(), room.pad.as_str()), ("r2", "Left"));
    session.logout();

    let events = harness.finish();
    assert_eq!(closed_with(&events), Some(Disconnect::LoggedOut));
    assert!(events.contains(&Event::Moved {
        cell: "r2".into(),
        pad: "Left".into(),
    }));
}

#[test]
fn a_move_sent_during_a_join_runs_after_the_join() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "yulgar"));
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::retrieve_user_datas(AreaId(43), &[EntityId(8)]));
        peer.expect(&packet::move_to_cell(AreaId(43), "r2", "Left"));
        peer.expect_logout();
    });
    let joined = harness.session.join("yulgar");
    let moved = harness.session.move_to_cell("r2", "Left");
    joined.wait().unwrap();
    moved.wait().unwrap();
    harness.session.logout();
    harness.finish();
}

#[test]
fn the_snapshot_shows_a_joined_room_before_its_event_arrives() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "yulgar"));
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect_logout();
    });
    let joined = harness.session.join("yulgar");
    let room_at_event = harness.events.iter().find_map(|event| match event {
        Event::Joined { map, .. } if map == "yulgar-2" => Some(
            harness
                .session
                .snapshot()
                .room
                .as_ref()
                .map(|r| r.name.clone()),
        ),
        _ => None,
    });
    assert_eq!(room_at_event, Some(Some("yulgar-2".into())));
    joined.wait().unwrap();
    harness.session.logout();
    harness.finish();
}

#[test]
fn a_refused_join_reports_the_warning_and_the_next_join_waits_out_the_cooldown() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "doomvault"));
        let refused_at = Instant::now();
        peer.send("%xt%warning%-1%Cannot join that map.%");
        peer.expect(&packet::transfer("player1", "yulgar"));
        assert!(refused_at.elapsed() >= TRANSFER_COOLDOWN - Duration::from_millis(100));
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect_logout();
    });
    let refused = harness.session.join("doomvault").wait().unwrap_err();
    assert!(matches!(refused, ActionError::Refused(why) if why == "Cannot join that map."));
    harness.session.join("yulgar").wait().unwrap();
    assert_eq!(
        harness.session.snapshot().room.as_ref().unwrap().name,
        "yulgar-2"
    );
    harness.session.logout();
    harness.finish();
}

#[test]
fn a_logout_cancels_a_join_waiting_out_the_cooldown() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "doomvault"));
        peer.send("%xt%warning%-1%Cannot join that map.%");
        peer.expect(&packet::logout());
    });
    harness.session.join("doomvault").wait().unwrap_err();
    let cooling = harness.session.join("yulgar");
    thread::sleep(Duration::from_millis(100));
    harness.session.logout();
    let started = Instant::now();
    let joined = cooling.wait();
    assert!(
        matches!(joined, Err(ActionError::Closed(Disconnect::LoggedOut))),
        "{joined:?}"
    );
    assert!(started.elapsed() < TRANSFER_COOLDOWN / 2);
    harness.finish();
}

#[test]
fn a_logout_cancels_a_sent_join_even_if_the_server_confirms_it() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "yulgar"));
        peer.expect(&packet::logout());
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
    });
    let sent = harness.session.join("yulgar");
    thread::sleep(Duration::from_millis(100));
    harness.session.logout();
    let joined = sent.wait();
    assert!(
        matches!(joined, Err(ActionError::Closed(Disconnect::LoggedOut))),
        "{joined:?}"
    );
    harness.finish();
}

#[test]
fn a_pending_join_reports_the_closed_session_when_the_server_drops() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "yulgar"));
    });
    let joined = harness.session.join("yulgar").wait();
    assert!(
        matches!(
            joined,
            Err(ActionError::Closed(Disconnect::ConnectionLost(_)))
        ),
        "{joined:?}"
    );
    harness.finish();
}

#[test]
fn a_move_after_the_session_ends_reports_why_it_closed() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        thread::sleep(Duration::from_millis(100));
    });
    harness.session.ready().wait().unwrap();
    let session = harness.session.clone();
    harness.finish();
    let moved = session.move_to_cell("r3", "Left").wait();
    assert!(
        matches!(
            moved,
            Err(ActionError::Closed(Disconnect::ConnectionLost(_)))
        ),
        "{moved:?}"
    );
}

#[test]
fn tracks_players_entering_moving_and_leaving_the_room() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(
            "<msg t='sys'><body action='uER' r='42'><u i='10' m='0' s='0' p='3'>\
             <n><![CDATA[player4]]></n><vars></vars></u></body></msg>",
        );
        peer.expect(&packet::retrieve_user_data(AreaId(42), EntityId(10)));
        peer.send("%xt%uotls%-1%player4%strFrame:r2,strPad:Left%");
        peer.send("%xt%exitArea%-1%8%player2%");
        peer.send("<msg t='sys'><body action='userGone' r='42'><user id='8' /></body></msg>");
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert_eq!(closed_with(&events), Some(Disconnect::LoggedInElsewhere));
    let players: Vec<(String, Option<String>)> = session
        .snapshot()
        .room
        .as_ref()
        .unwrap()
        .players
        .iter()
        .map(|p| (p.name.clone(), p.cell.clone()))
        .collect();
    assert_eq!(
        players,
        [
            ("player3".into(), Some("r2".into())),
            ("player4".into(), Some("r2".into())),
        ]
    );
    assert!(events.contains(&Event::PlayerEntered {
        name: "player4".into()
    }));
    let left: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::PlayerLeft { .. }))
        .collect();
    assert_eq!(
        left,
        [&Event::PlayerLeft {
            name: "player2".into()
        }]
    );
}

#[test]
fn tracks_the_rooms_monsters_through_combat_updates() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({
            "cmd": "moveToArea",
            "areaId": 43,
            "areaName": "farm-1",
            "uoBranch": [{"entID": 7, "strUsername": "player1", "strFrame": "Enter"}],
            "mondef": [
                {"MonID": 10, "strMonName": "Mosquito"},
                {"MonID": 11, "strMonName": "Frogzard"},
            ],
            "monmap": [
                {"MonMapID": 1, "MonID": 10, "strFrame": "Enter", "bRed": "1"},
                {"MonMapID": 2, "MonID": 11, "strFrame": "r2", "bRed": 0},
            ],
            "monBranch": [
                {"MonMapID": 1, "MonID": 10, "intHP": 540, "intHPMax": 540, "intState": 1},
                {"MonMapID": 2, "MonID": 11, "intHP": "800", "intHPMax": "800", "intState": "1"},
            ],
        })));
        peer.send("%xt%server%-1%You joined \"farm-1\"!%");
        peer.send(&json_message(
            json!({"cmd": "cb", "m": {"1": {"intState": 2}}}),
        ));
        peer.send(&json_message(
            json!({"cmd": "mtls", "id": 1, "o": {"intHP": "300"}}),
        ));
        peer.send(&json_message(json!({
            "cmd": "ct",
            "m": {"2": {"intHP": 0, "intState": 0}},
            "p": {"player1": {"intHP": 1000}},
        })));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    let world = session.snapshot();
    let room = world.room.as_ref().unwrap();
    assert_eq!(room.name, "farm-1");
    assert_eq!(
        room.monsters,
        [
            Monster {
                id: MonMapId(1),
                name: "Mosquito".into(),
                cell: "Enter".into(),
                hp: 300,
                max_hp: 540,
                state: CombatState::InCombat,
                aggressive: true,
            },
            Monster {
                id: MonMapId(2),
                name: "Frogzard".into(),
                cell: "r2".into(),
                hp: 0,
                max_hp: 800,
                state: CombatState::Dead,
                aggressive: false,
            },
        ]
    );
}

#[test]
fn tracks_the_players_vitals_haste_and_skills() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({
            "cmd": "moveToArea",
            "areaId": 43,
            "areaName": "farm-1",
            "uoBranch": [{
                "entID": 7, "strUsername": "player1", "strFrame": "Enter",
                "intHP": 900, "intHPMax": "1000", "intMP": 80, "intMPMax": 100, "intState": 1,
            }],
        })));
        peer.send("%xt%server%-1%You joined \"farm-1\"!%");
        peer.send(&json_message(
            json!({"cmd": "stu", "sta": {"$tha": 0.12, "$STR": 50}}),
        ));
        peer.send(&json_message(json!({"cmd": "sAct", "actions": {"active": [
            {"ref": "aa", "nam": "Auto Attack", "cd": 2000, "mp": -10, "tgt": "h", "tgtMax": 1},
            {"ref": "a1", "nam": "Guard", "cd": "3000", "mp": 15, "tgt": "s", "tgtMax": "3", "isOK": false},
            {"ref": "i1", "nam": "Health Potion", "cd": 60000, "mp": 0, "tgt": "s", "tgtMax": null},
        ]}})));
        peer.send(&json_message(json!({
            "cmd": "ct",
            "p": {"player1": {"intHP": 700, "intState": 2}, "player2": {"intHP": 1}},
        })));
        peer.send(&json_message(
            json!({"cmd": "uotls", "unm": "Player1", "o": {"intMP": 60}}),
        ));
        peer.send(&json_message(
            json!({"cmd": "uotls", "unm": "player2", "o": {"intMP": 1}}),
        ));
        peer.send(&json_message(json!({"cmd": "stu", "sta": {"$tdo": 0.1}})));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    assert_eq!(
        session.snapshot().me,
        Some(Me {
            level: 50,
            exp: 400,
            exp_to_level: 2000,
            gold: 1200,
            coins: 30,
            member: false,
            class_points: 899,
            hp: 700,
            max_hp: 1000,
            mp: 60,
            max_mp: 100,
            state: CombatState::InCombat,
            haste: 0.12,
            quests: session.snapshot().me.as_ref().unwrap().quests.clone(),
            skills: vec![
                Skill {
                    slot: "aa".into(),
                    name: "Auto Attack".into(),
                    cooldown_ms: 2000,
                    mana: -10,
                    target: SkillTarget::Hostile,
                    max_targets: 1,
                    unlocked: true,
                },
                Skill {
                    slot: "a1".into(),
                    name: "Guard".into(),
                    cooldown_ms: 3000,
                    mana: 15,
                    target: SkillTarget::Myself,
                    max_targets: 3,
                    unlocked: false,
                },
                Skill {
                    slot: "i1".into(),
                    name: "Health Potion".into(),
                    cooldown_ms: 60000,
                    mana: 0,
                    target: SkillTarget::Myself,
                    max_targets: 1,
                    unlocked: true,
                },
            ],
        })
    );
}

#[test]
fn tracks_inventory_and_temp_items() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({"cmd": "addItems", "items": {
            "101": {"CharItemID": 5001, "iQty": 2, "iQtyNow": 7, "bBank": 0},
            "102": {"ItemID": 102, "CharItemID": 5002, "sName": "Iron Axe", "iQty": 1, "iStk": 1,
                    "sType": "Axe", "sES": "Weapon"},
            "103": {"ItemID": 103, "CharItemID": 5003, "sName": "Rogue", "iQty": 302500, "iStk": 1,
                    "sType": "Class", "sES": "ar"},
            "200": {"ItemID": 200, "sName": "Mosquito Wing", "iQty": 1, "iStk": 5,
                    "sType": "Quest Item", "bTemp": true},
            "201": {"ItemID": 201, "sName": "Frog Leg", "iQty": 1, "iStk": 5, "bTemp": true},
            "300": {"CharItemID": 9000, "iQty": 1, "iQtyNow": 20, "bBank": 1},
        }})));
        peer.send(&json_message(
            json!({"cmd": "addItems", "items": {"200": {"iQty": 2}}}),
        ));
        peer.send(&json_message(
            json!({"cmd": "equipItem", "uid": 7, "ItemID": 102}),
        ));
        peer.send(&json_message(
            json!({"cmd": "equipItem", "uid": 8, "ItemID": 100}),
        ));
        peer.send(&json_message(
            json!({"cmd": "turnIn", "sItems": "200:1,201:1"}),
        ));
        peer.send(&json_message(
            json!({"cmd": "sellItem", "CharItemID": 5.001E3, "iQty": 3, "intAmount": 30}),
        ));
        peer.send(&json_message(
            json!({"cmd": "sellItem", "CharItemID": 5003, "iQty": 1, "intAmount": 0}),
        ));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    let world = session.snapshot();
    let item =
        |id, char_item_id, name: &str, qty, max_stack, kind: &str, slot: &str, equipped| Item {
            id: ItemId(id),
            char_item_id: CharItemId(char_item_id),
            name: name.into(),
            qty,
            max_stack,
            kind: kind.into(),
            slot: slot.into(),
            equipped,
            coins: false,
            member: false,
        };
    assert_eq!(
        *world.inventory,
        [
            item(100, 5000, "Default Sword", 1, 1, "Sword", "Weapon", false),
            item(101, 5001, "Health Potion", 4, 10, "Item", "None", false),
            item(102, 5002, "Iron Axe", 1, 1, "Axe", "Weapon", true),
        ]
    );
    assert_eq!(
        world.temp,
        [TempItem {
            id: ItemId(200),
            name: "Mosquito Wing".into(),
            qty: 2,
            max_stack: 5,
        }]
    );
}

#[test]
fn a_malformed_inventory_item_is_skipped_without_blocking_login() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_with_items(
            "5.31",
            json!([
                {"ItemID": 100, "sName": "Broken"},
                {"ItemID": 101, "CharItemID": 5001, "sName": "Health Potion", "iQty": 5},
            ]),
        );
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    let names: Vec<String> = harness
        .session
        .snapshot()
        .inventory
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, ["Health Potion"]);
    harness.session.logout();
    let events = harness.finish();
    assert!(
        events.iter().any(
            |e| matches!(e, Event::Unreadable { error } if error.starts_with("inventory item"))
        )
    );
}

#[test]
fn tracks_gold_exp_levels_and_drops() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(
            json!({"cmd": "addGoldExp", "intGold": 25, "intExp": 100, "typ": "m", "id": 1}),
        ));
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {
            "300": {"ItemID": 300, "sName": "Treeant Branch", "iQty": 1, "iStk": 50,
                    "sType": "Item", "sES": "None"},
            "101": {"ItemID": 101, "sName": "Health Potion", "iQty": 2, "iStk": 10,
                    "sType": "Item", "sES": "None"},
        }})));
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {
            "300": {"ItemID": 300, "sName": "Treeant Branch", "iQty": 2, "iStk": 50},
            "101": {"ItemID": 101, "sName": "Health Potion", "iQty": 1, "iStk": 10},
        }})));
        peer.send(&json_message(json!({
            "cmd": "getDrop", "bSuccess": 1, "ItemID": 300, "CharItemID": 6000,
            "iQty": 3, "iQtyNow": 3, "bBank": 0,
        })));
        peer.send(&json_message(
            json!({"cmd": "getDrop", "bSuccess": 0, "ItemID": 101}),
        ));
        peer.send(&json_message(
            json!({"cmd": "levelUp", "intLevel": 51, "intExpToLevel": 2500, "xp": 100}),
        ));
        peer.send(&json_message(
            json!({"cmd": "addGoldExp", "intGold": 0, "intExp": 50, "typ": "m", "id": 1}),
        ));
        peer.send(&json_message(
            json!({"cmd": "sellItem", "CharItemID": 5000, "iQty": 1, "intAmount": 2, "bCoins": 1}),
        ));
        peer.send(&json_message(
            json!({"cmd": "sellItem", "CharItemID": 5001, "iQty": 1, "intAmount": 10, "bCoins": 0}),
        ));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    let world = session.snapshot();
    let me = world.me.as_ref().unwrap();
    assert_eq!(
        (me.level, me.exp, me.exp_to_level, me.gold, me.coins),
        (51, 150, 2500, 1235, 32)
    );
    let held: Vec<(&str, u32)> = world
        .inventory
        .iter()
        .map(|i| (i.name.as_str(), i.qty))
        .collect();
    assert_eq!(held, [("Health Potion", 4), ("Treeant Branch", 3)]);
    assert_eq!(world.inventory[1].char_item_id, CharItemId(6000));
    assert_eq!(
        world.drops,
        [DropOffer {
            id: ItemId(101),
            name: "Health Potion".into(),
            qty: 3,
            max_stack: 10,
            kind: "Item".into(),
            slot: "None".into(),
            coins: false,
            member: false,
        }]
    );
}

#[test]
fn tracks_faction_reputation_and_rank() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({
            "cmd": "addGoldExp", "intGold": 0, "intExp": 0, "iRep": 2600, "bonusRep": 100,
            "FactionID": 46, "typ": "q",
        })));
        peer.send(&json_message(json!({"cmd": "addFaction", "faction": {
            "FactionID": "23", "CharFactionID": "901", "sName": "Evil", "iRep": "0",
        }})));
        peer.send(&json_message(
            json!({"cmd": "addGoldExp", "intGold": 5, "intExp": 5, "iRep": 50, "FactionID": 99}),
        ));
        peer.send(&json_message(
            json!({"cmd": "addGoldExp", "intGold": 5, "intExp": 5, "typ": "m", "id": 1}),
        ));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    assert_eq!(
        session.snapshot().factions,
        [
            Faction {
                id: FactionId(46),
                name: "Good".into(),
                rep: 3600,
                rank: 3,
            },
            Faction {
                id: FactionId(23),
                name: "Evil".into(),
                rep: 0,
                rank: 1,
            },
        ]
    );
}

#[test]
fn tracks_quest_progress_and_achievements() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        for (slot, value) in [(3, 35), (250, 1), (151, 40), (152, -1)] {
            peer.send(&json_message(
                json!({"cmd": "updateQuest", "iIndex": slot, "iValue": value}),
            ));
        }
        peer.send(&json_message(
            json!({"cmd": "setAchievement", "field": "ia0", "index": 2, "value": 0}),
        ));
        peer.send(&json_message(
            json!({"cmd": "setAchievement", "field": "ia1", "index": "4", "value": 1}),
        ));
        peer.send(&json_message(
            json!({"cmd": "setAchievement", "field": "id1", "index": 0, "value": 2}),
        ));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    let world = session.snapshot();
    let quests = &world.me.as_ref().unwrap().quests;
    let values: Vec<Option<u32>> = [2, 3, 150, 151, 152, 250, 900]
        .into_iter()
        .map(|slot| quests.value(slot))
        .collect();
    assert_eq!(
        values,
        [Some(10), Some(35), Some(1), Some(0), Some(0), None, None]
    );
    let flags: Vec<Option<bool>> = [
        ("ia0", 0),
        ("ia0", 2),
        ("iw0", 3),
        ("im0", 31),
        ("ia1", 4),
        ("id0", 0),
        ("id1", 0),
        ("ia0", 32),
    ]
    .into_iter()
    .map(|(field, bit)| quests.achievement(field, bit))
    .collect();
    assert_eq!(
        flags,
        [
            Some(true),
            Some(false),
            Some(true),
            Some(true),
            Some(true),
            None,
            None,
            None
        ]
    );
}

#[test]
fn quest_status_follows_the_game_clients_checks() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let quest = |id: i64, name: &str, extra: Value| {
            let mut quest = json!({"QuestID": id, "sName": name});
            quest
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            quest
        };
        let quests = json!({
            "1": quest(1, "Once done", json!({"bOnce": 1, "iSlot": 2, "iValue": 10})),
            "2": quest(2, "Once open", json!({
                "bOnce": "1", "iSlot": 2, "iValue": 11, "iGold": 100, "iExp": 200,
                "turnin": [{"ItemID": 101, "iQty": 3}],
                "oItems": {"101": {"ItemID": 101, "sName": "Health Potion"}},
                "reward": [
                    {"ItemID": 500, "iQty": 1, "iRate": "100", "iType": 0},
                    {"ItemID": "501", "iQty": 2, "iRate": 12.5, "iType": 1},
                ],
                "oRewards": {
                    "itemsS": {"500": {"ItemID": 500, "sName": "Gold Bar"}},
                    "itemsR": {"501": {"ItemID": "501", "sName": "Gem"}},
                },
            })),
            "3": quest(3, "Weekly", json!({"sField": "iw0", "iIndex": 3})),
            "4": quest(4, "Daily, field missing", json!({"sField": "id5", "iIndex": 0})),
            "5": quest(5, "Members only", json!({"bUpg": 1})),
            "6": quest(6, "Chain", json!({"iSlot": 1, "iValue": 2})),
            "7": quest(7, "High level", json!({"iLvl": 60})),
            "8": quest(8, "Class missing", json!({"iClass": 999, "iReqCP": 0})),
            "9": quest(9, "Rep met", json!({"FactionID": 46, "iReqRep": 900})),
            "10": quest(10, "Rep short", json!({"FactionID": 46, "iReqRep": 3600})),
            "11": quest(11, "Holds item", json!({"reqd": [{"ItemID": 101, "iQty": 5}]})),
            "12": quest(12, "Lacks item", json!({"reqd": [{"ItemID": 101, "iQty": 6}]})),
            "13": quest(13, "No faction", json!({"FactionID": 1, "iReqRep": 99999})),
            "14": quest(14, "Past string end", json!({"iSlot": 160, "iValue": 1})),
            "15": quest(15, "String missing", json!({"iSlot": 250, "iValue": 1})),
            "16": quest(16, "Broken", json!({"reward": [{"ItemID": 1, "iType": 9}]})),
            "17": quest(17, "Class rank met", json!({"iClass": 700, "iReqCP": 900})),
            "18": quest(18, "Class rank short", json!({"iClass": 700, "iReqCP": 901})),
        });
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"700": {
            "ItemID": 700, "CharItemID": 5700, "sName": "Rogue", "iQty": 899, "iStk": 1,
            "sType": "Class", "sES": "ar",
        }}})));
        peer.send(&json_message(
            json!({"cmd": "equipItem", "uid": 7, "ItemID": 700}),
        ));
        peer.send(&json_message(
            json!({"cmd": "updateClass", "uid": 8, "sClassName": "Healer", "iCP": 5}),
        ));
        peer.send(&json_message(
            json!({"cmd": "addGoldExp", "intGold": 0, "intExp": 0, "iCP": 1, "typ": "m", "id": 1}),
        ));
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": quests})));
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {
            "2": {"QuestID": 2, "sName": "Changed"},
        }})));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    let unreadable: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::Unreadable { .. }))
        .collect();
    assert!(
        matches!(unreadable[..], [Event::Unreadable { error }] if error.starts_with("quest 16")),
        "{unreadable:?}"
    );
    let world = session.snapshot();
    let statuses: Vec<(i64, Option<QuestStatus>)> = (1..=18)
        .map(|id| (id, world.quest_status(QuestId(id))))
        .collect();
    use QuestStatus::*;
    assert_eq!(
        statuses,
        [
            (1, Some(Done)),
            (2, Some(Available)),
            (3, Some(DoneThisPeriod)),
            (4, Some(DoneThisPeriod)),
            (5, Some(Locked)),
            (6, Some(Locked)),
            (7, Some(Locked)),
            (8, Some(Locked)),
            (9, Some(Available)),
            (10, Some(Locked)),
            (11, Some(Available)),
            (12, Some(Locked)),
            (13, Some(Available)),
            (14, Some(Available)),
            (15, Some(Locked)),
            (16, None),
            (17, Some(Available)),
            (18, Some(Locked)),
        ]
    );
    assert_eq!(world.me.as_ref().unwrap().class_points, 900);
    assert_eq!(
        world.quests[&QuestId(2)],
        Quest {
            id: QuestId(2),
            name: "Once open".into(),
            turn_in: vec![QuestItem {
                id: ItemId(101),
                name: "Health Potion".into(),
                qty: 3,
            }],
            required: vec![],
            rewards: vec![
                QuestReward {
                    id: ItemId(500),
                    name: "Gold Bar".into(),
                    qty: 1,
                    chance: 100.0,
                    kind: RewardKind::Always,
                },
                QuestReward {
                    id: ItemId(501),
                    name: "Gem".into(),
                    qty: 2,
                    chance: 12.5,
                    kind: RewardKind::Rolled,
                },
            ],
            once: true,
            slot: 2,
            value: 11,
            period: None,
            member: false,
            level: 0,
            class: None,
            required_cp: 0,
            faction: None,
            required_rep: 0,
            gold: 100,
            exp: 200,
        }
    );
}

#[test]
fn packet_trace_hides_the_token_and_personal_fields() {
    let options = Options {
        trace_packets: true,
        ..Options::default()
    };
    let harness = Harness::start(options, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect_logout();
    });
    harness.session.ready().wait().unwrap();
    harness.session.logout();

    let traced: Vec<(Direction, String)> = harness
        .finish()
        .into_iter()
        .filter_map(|e| match e {
            Event::Packet { direction, text } => Some((direction, text)),
            _ => None,
        })
        .collect();
    for secret in [TOKEN, "player1@example.com", "192.0.2.1"] {
        assert!(
            traced.iter().all(|(_, text)| !text.contains(secret)),
            "{secret} leaked"
        );
    }
    assert!(
        traced
            .iter()
            .any(|(d, t)| *d == Direction::Out && t.contains("<token>"))
    );
    assert!(
        traced
            .iter()
            .any(|(d, t)| *d == Direction::In && t.contains("\"intLevel\":\"50\""))
    );
    assert!(
        traced
            .iter()
            .any(|(d, t)| *d == Direction::In && t.contains("loadInventoryBig"))
    );
}

#[test]
fn phase_events_serialize_flat() {
    let json = |e: Event| serde_json::to_value(e).unwrap();
    assert_eq!(
        json(Event::Phase(Phase::Loading {
            step: Load::Joining
        })),
        json!({"event": "phase", "phase": "loading", "step": "joining"})
    );
    assert_eq!(
        json(Event::Phase(Phase::Closed {
            reason: Disconnect::LoggedOut
        })),
        json!({"event": "phase", "phase": "closed", "reason": "logged out"})
    );
}
