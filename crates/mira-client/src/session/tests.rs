use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde_json::json;

use super::*;

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
            "data": {"intLevel": "50", "intGold": 1200, "strEmail": "player1@example.com", "strIP": "192.0.2.1"},
        })));
        self.expect(&packet::retrieve_inventory(AreaId(42), EntityId(7)));
        let inventory =
            json_message(json!({"cmd": "loadInventoryBig", "items": [{}, {}], "factions": []}));
        self.send_bytes(&compress(&inventory));
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
            {"entID": 7, "strUsername": "player1", "strFrame": "Enter"},
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
