use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
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
    session: Session,
    events: Rc<RefCell<Vec<Event>>>,
    server: Server,
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
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let session = Session::new(&account, options, move |e: &Event| {
            sink.borrow_mut().push(e.clone())
        })
        .unwrap();
        Harness {
            session,
            events,
            server,
            script: thread::spawn(move || script(listener)),
        }
    }

    fn login(&mut self) -> Result<(), LoginError> {
        let server = self.server.clone();
        self.session.login(&server)
    }

    fn finish(self) -> Vec<Event> {
        drop(self.session);
        self.script.join().expect("the fake server failed");
        self.events.take()
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

#[test]
fn logs_in_reports_each_phase_and_logs_out() {
    let mut harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::logout());
    });
    harness.login().unwrap();
    harness.session.control().logout();
    assert_eq!(harness.session.run(), Disconnect::LoggedOut);

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
    let mut harness = Harness::start(Options::default(), |listener| {
        let mut outdated = Peer::accept(&listener);
        outdated.expect(&packet::version_check());
        outdated.send(API_OK);
        outdated.expect(&packet::login("player1", "5.31", TOKEN));
        outdated.send(OUTDATED);
        outdated.wait_for_close();
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.40");
        peer.wait_for_close();
    });
    harness.login().unwrap();

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
fn a_pinned_version_is_not_retried() {
    let options = Options {
        client_version: Some("5.20".into()),
        ..Options::default()
    };
    let mut harness = Harness::start(options, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.expect(&packet::version_check());
        peer.send(API_OK);
        peer.expect(&packet::login("player1", "5.20", TOKEN));
        peer.send(OUTDATED);
        peer.wait_for_close();
    });
    let err = harness.login().unwrap_err();
    assert!(matches!(err, LoginError::Closed(Disconnect::ClientVersion(v)) if v == "5.40"));
    harness.finish();
}

#[test]
fn a_rejected_login_reports_the_servers_reason() {
    let mut harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.expect(&packet::version_check());
        peer.send(API_OK);
        peer.recv();
        peer.send("%xt%loginResponse%-1%0%-1%%Server is full.%");
        peer.wait_for_close();
    });
    let err = harness.login().unwrap_err();
    assert!(matches!(err, LoginError::Closed(Disconnect::Rejected(m)) if m == "Server is full."));
    harness.finish();
}

#[test]
fn later_joins_skip_own_data_and_chat_flows_until_logged_in_elsewhere() {
    let mut harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::retrieve_user_datas(AreaId(43), &[EntityId(8)]));
        peer.send("%xt%chatm%-1%zone~hello there%player2%");
        peer.send("%xt%multiLoginWarning%-1%");
    });
    harness.login().unwrap();
    assert_eq!(harness.session.run(), Disconnect::LoggedInElsewhere);

    let events = harness.finish();
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
fn packet_trace_hides_the_token_and_personal_fields() {
    let options = Options {
        trace_packets: true,
        ..Options::default()
    };
    let mut harness = Harness::start(options, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.wait_for_close();
    });
    harness.login().unwrap();

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
