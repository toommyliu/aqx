use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde_json::json;

use mira_protocol::{
    AreaId, CharItemId, FactionId, ItemId, MapItemId, MonMapId, ShopId, ShopItemId,
};

use super::*;
use crate::Place;
use crate::quest::{QuestItem, QuestReward, QuestStatus, RewardKind};
use crate::{
    Aura, Auras, BoostKind, Caster, CombatState, DropOffer, Item, Monster, Skill, SkillTarget,
    StatEffect, TempItem,
};

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
                "iUpgDays": -1, "iCP": 899, "iBoostRep": 120, "iBagSlots": 2,
                "CharID": "4242", "iBankSlots": 3,
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
            json!({"cmd": "loadInventoryBig", "items": items, "bankCount": 1, "factions": [
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
            {"entID": 9, "strUsername": "player3", "strFrame": "r2", "intLevel": 10},
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
        Harness::start_with_api(options, "http://127.0.0.1:9".into(), script)
    }

    fn start_with_api(
        options: Options,
        api: String,
        script: impl FnOnce(TcpListener) + Send + 'static,
    ) -> Harness {
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
            device: crate::api::DeviceId::for_tests(),
            api,
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
fn a_refused_join_reports_the_warning_and_the_next_join_waits_out_the_cooldown_and_backoff() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "doomvault"));
        let refused_at = Instant::now();
        peer.send("%xt%warning%-1%Cannot join that map.%");
        thread::sleep(Duration::from_millis(300));
        peer.send("%xt%warning%-1%Please slow down.%");
        peer.expect(&packet::transfer("player1", "yulgar"));
        let gap = Limit::Transfer.gap() + combat::BACKOFF_STEP;
        assert!(refused_at.elapsed() >= gap - Duration::from_millis(100));
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
    assert!(started.elapsed() < Limit::Transfer.gap() / 2);
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
                auras: Auras::default(),
            },
            Monster {
                id: MonMapId(2),
                name: "Frogzard".into(),
                cell: "r2".into(),
                hp: 0,
                max_hp: 800,
                state: CombatState::Dead,
                aggressive: false,
                auras: Auras::default(),
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
            json!({"cmd": "stu", "sta": {"$tha": 0.12, "$cmc": 0.8, "$STR": 50}}),
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
            class_name: String::new(),
            class_points: 899,
            char_id: 4242,
            bag_slots: 2,
            bank_slots: 3,
            bank_count: 1,
            item_slot: None,
            boosts: session.snapshot().me.as_ref().unwrap().boosts.clone(),
            hp: 700,
            max_hp: 1000,
            mp: 60,
            max_mp: 100,
            state: CombatState::InCombat,
            haste: 0.12,
            mana_rate: Some(0.8),
            quests: session.snapshot().me.as_ref().unwrap().quests.clone(),
            auras: Auras::default(),
            skills: vec![
                Skill {
                    slot: "aa".into(),
                    name: "Auto Attack".into(),
                    cooldown_ms: 2000,
                    mana: -10,
                    target: SkillTarget::Hostile,
                    max_targets: 1,
                    unlocked: true,
                    old_cooldown_ms: None,
                },
                Skill {
                    slot: "a1".into(),
                    name: "Guard".into(),
                    cooldown_ms: 3000,
                    mana: 15,
                    target: SkillTarget::Myself,
                    max_targets: 3,
                    unlocked: false,
                    old_cooldown_ms: None,
                },
                Skill {
                    slot: "i1".into(),
                    name: "Health Potion".into(),
                    cooldown_ms: 60000,
                    mana: 0,
                    target: SkillTarget::Myself,
                    max_targets: 1,
                    unlocked: true,
                    old_cooldown_ms: None,
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
            meta: String::new(),
            link: String::new(),
            house: false,
            enhancement: None,
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
            meta: String::new(),
            link: String::new(),
            house: false,
            enhancement: Default::default(),
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
            "19": quest(19, "Holds temp item", json!({"reqd": [{"ItemID": 800, "iQty": 2}]})),
        });
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"800": {
            "ItemID": 800, "sName": "Quest Token", "iQty": 2, "iStk": 10, "bTemp": true,
        }}})));
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
    let statuses: Vec<(i64, Option<QuestStatus>)> = (1..=19)
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
            (19, Some(Available)),
        ]
    );
    assert_eq!(world.me.as_ref().unwrap().class_points, 900);
    let class = world
        .inventory
        .iter()
        .find(|i| i.id == ItemId(700))
        .unwrap();
    assert_eq!(
        (class.class_rank(), world.inventory[0].class_rank()),
        (Some(2), None)
    );
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
fn tracks_auras_the_way_the_game_client_stores_them() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let monster = |id: i64, cell: &str| {
            (
                json!({"MonMapID": id, "MonID": 10, "strFrame": cell}),
                json!({"MonMapID": id, "MonID": 10, "intHP": 100, "intHPMax": 100, "intState": 1}),
            )
        };
        let monsters = [monster(1, "Enter"), monster(2, "r2"), monster(3, "Enter")];
        peer.send(&json_message(json!({
            "cmd": "moveToArea", "areaId": 43, "areaName": "farm-1",
            "uoBranch": [{"entID": 7, "strUsername": "player1", "strFrame": "Enter"}],
            "mondef": [{"MonID": 10, "strMonName": "Frogzard"}],
            "monmap": monsters.iter().map(|m| m.0.clone()).collect::<Vec<_>>(),
            "monBranch": monsters.iter().map(|m| m.1.clone()).collect::<Vec<_>>(),
        })));
        peer.send("%xt%server%-1%You joined \"farm-1\"!%");
        let add = |caster: &str, target: &str, auras: Value| json!({"cmd": "aura+", "cInf": caster, "tInf": target, "auras": auras});
        peer.send(&json_message(json!({
            "cmd": "ct",
            "m": {"1": {"intState": 2}},
            "a": [
                add("p:7", "m:1", json!([{"nam": "Bleed", "dur": 5, "isNew": true}])),
                add("p:7", "m:2", json!([{"nam": "Poison", "dur": 5, "stk": "3", "val": "12.5", "cat": "dot", "isNew": 1}])),
                add("p:7", "m:3", json!([{"nam": "Mark", "dur": 5, "isNew": true}])),
            ],
        })));
        let mut me = add(
            "m:1",
            "p:7",
            json!([
                {"nam": "Stun", "dur": 2, "isNew": true, "persist": true},
                {"nam": "Pact", "dur": 0, "isNew": true, "persist": true},
                {"nam": "Slow", "dur": 3, "isNew": true, "persist": true},
            ]),
        );
        peer.send(&json_message(me.clone()));
        me["auras"] = json!([{"nam": "Stun", "dur": 4, "val": 3, "isNew": false}]);
        peer.send(&json_message(me.clone()));
        me["auras"] = json!([{"nam": "Stun", "dur": 1, "isNew": true, "persist": true}]);
        peer.send(&json_message(me.clone()));
        me["auras"] = json!([{"nam": "Stun", "dur": 6, "val": 2, "isNew": false}]);
        peer.send(&json_message(me.clone()));
        me["cInf"] = json!("m:2");
        me["auras"] = json!([{"nam": "Stun", "dur": 9, "isNew": false}]);
        peer.send(&json_message(me));
        peer.send(&json_message(
            json!({"cmd": "aura+p", "cInf": "p:7", "tInf": "p:7", "auras": [{"nam": "Insight"}]}),
        ));
        peer.send(&json_message(
            json!({"cmd": "updateClass", "uid": 8, "sClassName": "Healer", "iCP": 5}),
        ));
        peer.send(&json_message(
            json!({"cmd": "aura+p", "tInf": "p:7", "auras": [{"nam": "Kept"}]}),
        ));
        peer.send(&json_message(
            json!({"cmd": "updateClass", "uid": 7, "sClassName": "Mage", "iCP": 41107}),
        ));
        peer.send(&json_message(
            json!({"cmd": "aura+p", "tInf": "p:7", "auras": [
                {"nam": "Hot Headed", "e": [
                    {"id": 83, "typ": "+", "sta": "cmo", "val": 0.1},
                    {"id": 19, "typ": "+", "sta": "thi", "val": "0.1"},
                ]},
            ]}),
        ));
        peer.send(&json_message(json!({"cmd": "sAct", "actions": {
            "active": [],
            "passive": [{"ref": "p1", "nam": "Hot Headed", "auras": [{}]}],
        }})));
        peer.send(&json_message(
            json!({"cmd": "aura-", "tInf": "p:7", "auras": [{"nam": "Pact"}]}),
        ));
        peer.send(&json_message(
            json!({"cmd": "aura-", "tInf": "p:7", "auras": [], "aura": {"nam": "Slow"}}),
        ));
        peer.send(&json_message(json!({"cmd": "clearAuras"})));
        peer.send(&json_message(add(
            "p:7",
            "p:8",
            json!([{"nam": "Elsewhere", "isNew": true}]),
        )));
        peer.send(&json_message(
            json!({"cmd": "ct", "m": {"1": {"intHP": 0}, "2": {"intHP": 0}}}),
        ));
        peer.send(&json_message(
            json!({"cmd": "mtls", "id": 3, "o": {"intState": 1}}),
        ));
        peer.send(&json_message(json!({
            "cmd": "cb",
            "m": {"3": {"intState": 2}},
            "a": [add("p:7", "m:3", json!([{"nam": "Late", "dur": 5, "isNew": true}]))],
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
    let aura = |name: &str, duration: f64, value: Option<f64>, caster| Aura {
        name: name.into(),
        duration,
        stacks: 1,
        value,
        category: None,
        persist: true,
        caster,
        effects: Vec::new(),
    };
    let from_monster = Some(Caster::Monster(MonMapId(1)));
    assert_eq!(world.me.as_ref().unwrap().class_points, 41107);
    assert_eq!(world.me.as_ref().unwrap().class_rank(), 5);
    assert_eq!(
        world.me.as_ref().unwrap().auras,
        Auras {
            active: vec![
                aura("Stun", 6.0, Some(2.0), from_monster),
                aura("Slow", 3.0, None, from_monster),
                aura("Stun", 6.0, Some(2.0), from_monster),
            ],
            passive: vec![Aura {
                persist: false,
                effects: vec![
                    StatEffect {
                        stat: "cmo".into(),
                        op: "+".into(),
                        value: 0.1,
                    },
                    StatEffect {
                        stat: "thi".into(),
                        op: "+".into(),
                        value: 0.1,
                    },
                ],
                ..aura("Hot Headed", 0.0, None, None)
            }],
        }
    );
    let monsters = &world.room.as_ref().unwrap().monsters;
    assert!(monsters[0].auras.active.is_empty());
    assert_eq!(
        monsters[1].auras.active,
        [Aura {
            name: "Poison".into(),
            duration: 5.0,
            stacks: 3,
            value: Some(12.5),
            category: Some("dot".into()),
            persist: false,
            caster: Some(Caster::Player(EntityId(7))),
            effects: Vec::new(),
        }]
    );
    assert_eq!(
        monsters[2].auras.active,
        [Aura {
            persist: false,
            caster: Some(Caster::Player(EntityId(7))),
            ..aura("Late", 5.0, None, None)
        }]
    );
}

#[test]
fn tracks_boosts_class_names_and_other_players() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({"cmd": "initUserDatas", "a": [
            {"uid": 8, "data": {"strUsername": "player2", "intLevel": 30, "strClassName": "Healer"}},
        ]})));
        peer.send(&json_message(
            json!({"cmd": "initUserData", "uid": 9, "data": {
                "strUsername": "player3", "intLevel": "45", "strClassName": "Rogue",
            }}),
        ));
        for (uid, class, points) in [(8, "Warrior", 5), (7, "Mage", 41107)] {
            peer.send(&json_message(json!({
                "cmd": "updateClass", "uid": uid, "sClassName": class, "iCP": points,
            })));
        }
        peer.send(&json_message(json!({
            "cmd": "uotls", "unm": "Player2", "o": {"intHP": 500, "intHPMax": 1000, "intState": 2},
        })));
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player3": {"intMP": 20}}}),
        ));
        peer.send(&json_message(
            json!({"cmd": "xpboost", "op": "+", "iSecsLeft": 3600, "bShowShop": false}),
        ));
        peer.send(&json_message(
            json!({"cmd": "gboost", "op": "+", "iSecsLeft": "60", "bShowShop": true}),
        ));
        peer.send(&json_message(json!({"cmd": "gboost", "op": "-"})));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let session = harness.session.clone();
    let events = harness.finish();
    assert!(
        !events.iter().any(|e| matches!(e, Event::Unreadable { .. })),
        "{events:?}"
    );
    let world = session.snapshot();
    let players: Vec<_> = world
        .room
        .as_ref()
        .unwrap()
        .players
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.level,
                p.class_name.as_deref(),
                p.hp,
                p.max_hp,
                p.mp,
                p.state,
            )
        })
        .collect();
    assert_eq!(
        players,
        [
            (
                "player2",
                Some(30),
                Some("Warrior"),
                500,
                1000,
                0,
                CombatState::InCombat
            ),
            (
                "player3",
                Some(45),
                Some("Rogue"),
                0,
                0,
                20,
                CombatState::Idle
            ),
        ]
    );
    let me = world.me.as_ref().unwrap();
    assert_eq!((me.class_name.as_str(), me.class_points), ("Mage", 41107));
    let now = Instant::now();
    let left = |kind| me.boosts.remaining(kind, now).as_secs();
    assert!(
        (3590..=3600).contains(&left(BoostKind::Xp)),
        "{}",
        left(BoostKind::Xp)
    );
    assert!(
        (110..=120).contains(&left(BoostKind::Rep)),
        "{}",
        left(BoostKind::Rep)
    );
    assert_eq!(
        (left(BoostKind::Gold), left(BoostKind::ClassPoints)),
        (0, 0)
    );
}

#[test]
fn reports_kills_deaths_levels_drops_and_item_changes() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({
            "cmd": "moveToArea", "areaId": 43, "areaName": "farm-1",
            "uoBranch": [{"entID": 7, "strUsername": "player1", "strFrame": "Enter"}],
            "mondef": [{"MonID": 10, "strMonName": "Mosquito"}],
            "monmap": [{"MonMapID": 1, "MonID": 10, "strFrame": "Enter"}],
            "monBranch": [{"MonMapID": 1, "MonID": 10, "intHP": 540, "intHPMax": 540, "intState": 1}],
        })));
        peer.send("%xt%server%-1%You joined \"farm-1\"!%");
        let send = |peer: &mut Peer, o: Value| peer.send(&json_message(o));
        send(
            &mut peer,
            json!({"cmd": "addGoldExp", "intGold": 5, "intExp": 10, "typ": "m", "id": 1}),
        );
        send(
            &mut peer,
            json!({"cmd": "addGoldExp", "intGold": 50, "intExp": 0, "typ": "q"}),
        );
        send(
            &mut peer,
            json!({"cmd": "dropItem", "items": {
                "300": {"ItemID": 300, "sName": "Branch", "iQty": 1, "iStk": 50},
            }}),
        );
        send(
            &mut peer,
            json!({
                "cmd": "getDrop", "bSuccess": 1, "ItemID": 300, "CharItemID": 6000,
                "iQty": 1, "iQtyNow": 1, "bBank": 0,
            }),
        );
        send(
            &mut peer,
            json!({"cmd": "addItems", "items": {
                "200": {"ItemID": 200, "sName": "Wing", "iQty": 2, "bTemp": true},
            }}),
        );
        send(&mut peer, json!({"cmd": "turnIn", "sItems": "200:2"}));
        send(
            &mut peer,
            json!({"cmd": "sellItem", "CharItemID": 5001, "iQty": 5, "intAmount": 5}),
        );
        send(
            &mut peer,
            json!({"cmd": "uotls", "unm": "player1", "o": {"intHP": 0}}),
        );
        send(
            &mut peer,
            json!({"cmd": "ct", "p": {"player1": {"intState": 0}}}),
        );
        send(
            &mut peer,
            json!({"cmd": "ct", "p": {"player1": {"intState": 0}}}),
        );
        send(
            &mut peer,
            json!({"cmd": "uotls", "unm": "player1", "o": {"intHP": 1000, "intState": 1}}),
        );
        send(
            &mut peer,
            json!({"cmd": "levelUp", "intLevel": 51, "intExpToLevel": 2500, "xp": 0}),
        );
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let events: Vec<Event> = harness
        .finish()
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Event::Killed { .. }
                    | Event::Died
                    | Event::Respawned
                    | Event::LeveledUp { .. }
                    | Event::DropOffered { .. }
                    | Event::ItemChanged { .. }
                    | Event::Unreadable { .. }
            )
        })
        .collect();
    let changed = |id, name: &str, qty, place| Event::ItemChanged {
        id: ItemId(id),
        name: name.into(),
        qty,
        place,
    };
    assert_eq!(
        events,
        [
            Event::Killed {
                id: MonMapId(1),
                name: "Mosquito".into(),
                gold: 5,
                exp: 10,
            },
            Event::DropOffered {
                id: ItemId(300),
                name: "Branch".into(),
                qty: 1,
            },
            changed(300, "Branch", 1, Place::Inventory),
            changed(200, "Wing", 2, Place::Temp),
            changed(200, "Wing", 0, Place::Temp),
            changed(101, "Health Potion", 0, Place::Inventory),
            Event::Died,
            Event::Respawned,
            Event::LeveledUp { level: 51 },
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

const FARM_SKILLS: &str = r#"{"cmd": "sAct", "actions": {"active": [
    {"ref": "aa", "nam": "Auto Attack", "cd": 2000, "mp": -10, "tgt": "h", "tgtMax": 1},
    {"ref": "a1", "nam": "Strike", "cd": 4000, "mp": 10, "tgt": "h", "tgtMax": 2},
    {"ref": "a2", "nam": "Shield", "cd": 4000, "mp": 5, "tgt": "s", "tgtMax": 1},
    {"ref": "a3", "nam": "Meteor", "cd": 4000, "mp": 999, "tgt": "h", "tgtMax": 1},
    {"ref": "a4", "nam": "Ultimate", "cd": 4000, "mp": 0, "tgt": "h", "tgtMax": 1, "isOK": false}
]}}"#;

impl Peer {
    fn serve_farm(&mut self) {
        self.serve_to_ready("5.31");
        self.send(&json_message(serde_json::from_str(FARM_SKILLS).unwrap()));
        self.send(&json_message(
            json!({"cmd": "stu", "sta": {"$tha": 0.84, "$cmc": 1}}),
        ));
        self.send(&json_message(json!({
            "cmd": "moveToArea", "areaId": 43, "areaName": "farm-1",
            "uoBranch": [{"entID": 7, "strUsername": "player1", "strFrame": "Enter",
                          "intHP": 1000, "intHPMax": 1000, "intMP": 100, "intMPMax": 100}],
            "mondef": [{"MonID": 10, "strMonName": "Mosquito"}, {"MonID": 11, "strMonName": "Frogzard"}],
            "monmap": [
                {"MonMapID": 1, "MonID": 10, "strFrame": "Enter", "bRed": 1},
                {"MonMapID": 2, "MonID": 10, "strFrame": "Enter", "bRed": 0},
                {"MonMapID": 3, "MonID": 11, "strFrame": "r2", "bRed": 0},
            ],
            "monBranch": [
                {"MonMapID": 1, "MonID": 10, "intHP": 500, "intHPMax": 500, "intState": 1},
                {"MonMapID": 2, "MonID": 10, "intHP": 200, "intHPMax": 500, "intState": 1},
                {"MonMapID": 3, "MonID": 11, "intHP": 800, "intHPMax": 800, "intState": 1},
            ],
        })));
        self.send("%xt%server%-1%You joined \"farm-1\"!%");
        self.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        self.expect(&packet::aggro_mon(AreaId(43), &[MonMapId(1)]));
    }

    fn expect_at(&mut self, packet: &str) -> Instant {
        self.expect(packet);
        Instant::now()
    }

    fn answer(&mut self, act: u8, res: u8) {
        self.send(&json_message(json!({
            "cmd": "ct",
            "sarsa": [{"actID": act, "cInf": "p:7", "iRes": res, "a": []}],
        })));
    }
}

fn gar(act: u8, slot: &str, aims: &[Aim]) -> String {
    packet::gar(act, slot, aims)
}

fn wait_for_farm(harness: &Harness) {
    harness
        .events
        .iter()
        .find(|e| matches!(e, Event::Joined { map, .. } if map == "farm-1"))
        .expect("the farm join");
}

const ONE: Aim = Aim::Monster(MonMapId(1));
const TWO: Aim = Aim::Monster(MonMapId(2));
const SLACK: Duration = Duration::from_millis(20);

#[test]
fn kills_a_monster_with_skills_and_the_auto_attack_at_the_clients_pace() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        let quick = FARM_SKILLS.replacen(r#""cd": 4000"#, r#""cd": 1000"#, 1);
        peer.send(&json_message(serde_json::from_str(&quick).unwrap()));
        let strike = peer.expect_at(&gar(0, "a1", &[ONE, TWO]));
        peer.answer(0, 1);
        let auto = peer.expect_at(&gar(1, "aa", &[ONE]));
        assert!(auto - strike >= Duration::from_millis(300) - SLACK);
        peer.answer(1, 1);
        let shield = peer.expect_at(&gar(2, "a2", &[Aim::Player(EntityId(7))]));
        assert!(shield - strike >= Duration::from_millis(750) - SLACK);
        assert!(shield - auto >= Duration::from_millis(700) - SLACK);
        peer.answer(2, 1);
        peer.send(&json_message(
            json!({"cmd": "ct", "m": {"1": {"intHP": 0, "intState": 0}}}),
        ));
        peer.expect(&packet::logout());
    });
    wait_for_farm(&harness);
    while harness.session.snapshot().me.as_ref().unwrap().skills[1].cooldown_ms != 1000 {
        thread::sleep(Duration::from_millis(5));
    }
    harness.session.kill(MonMapId(1)).wait().unwrap();
    thread::sleep(Duration::from_millis(1200));
    harness.session.logout();
    harness.finish();
}

#[test]
fn the_auto_attack_waits_for_a_class_skill_held_back_by_slow_down_warnings() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        peer.send("%xt%warning%-1%Please slow down%");
        peer.send("%xt%warning%-1%Please slow down%");
        let strike = peer.expect_at(&gar(0, "a1", &[ONE, TWO]));
        peer.answer(0, 1);
        let auto = peer.expect_at(&gar(1, "aa", &[ONE]));
        assert!(auto - strike >= Duration::from_millis(900) - SLACK);
        peer.answer(1, 1);
        let shield = peer.expect_at(&gar(2, "a2", &[Aim::Player(EntityId(7))]));
        assert!(shield - auto >= Duration::from_millis(1300) - SLACK);
        peer.send(&json_message(
            json!({"cmd": "ct", "m": {"1": {"intHP": 0, "intState": 0}}}),
        ));
        peer.expect_logout();
    });
    wait_for_farm(&harness);
    for _ in 0..2 {
        harness
            .events
            .iter()
            .find(|e| {
                matches!(
                    e,
                    Event::Notice {
                        kind: NoticeKind::Warning,
                        ..
                    }
                )
            })
            .expect("a warning");
    }
    harness.session.kill(MonMapId(1)).wait().unwrap();
    harness.session.logout();
    harness.finish();
}

fn wait_until(session: &Handle, done: impl Fn(&Me) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !session.snapshot().me.as_ref().is_some_and(&done) {
        assert!(Instant::now() < deadline, "the player never got there");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn leaving_combat_interrupts_the_fight_moves_away_from_aggressive_monsters_and_waits_for_the_server()
 {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        let fight =
            |state: u8| json_message(json!({"cmd": "cb", "p": {"player1": {"intState": state}}}));
        let calm = json_message(json!({"cmd": "uotls", "unm": "player1", "o": {"intState": 1}}));
        peer.expect(&gar(0, "a1", &[ONE, TWO]));
        peer.send(&fight(2));
        while peer.recv() != packet::move_to_cell(AreaId(43), "r2", "Left") {}
        peer.send(&calm);
        peer.expect(&packet::move_to_cell(AreaId(43), "r2", "Left"));
        peer.send(&fight(2));
        peer.expect(&packet::move_to_cell(AreaId(43), "r2", "Left"));
        thread::sleep(EXIT_TIMEOUT + Duration::from_millis(300));
        peer.send(&calm);
        peer.expect(&packet::logout());
    });
    wait_for_farm(&harness);
    let session = &harness.session;
    let in_combat = |me: &Me| me.state == CombatState::InCombat;
    let killing = session.kill(MonMapId(1));
    wait_until(session, in_combat);
    session.exit_combat().wait().unwrap();
    assert!(matches!(killing.wait(), Err(ActionError::Interrupted)));
    session.move_to_cell("r2", "Left").wait().unwrap();
    wait_until(session, in_combat);
    assert!(matches!(
        session.exit_combat().wait(),
        Err(ActionError::StillInCombat(cell)) if cell == "r2"
    ));
    wait_until(session, |me| me.state == CombatState::Idle);
    session.exit_combat().wait().unwrap();
    session.logout();
    harness.finish();
}

#[test]
fn a_refused_skill_reports_it_and_keeps_its_old_cooldown() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        let first = peer.expect_at(&gar(0, "a1", &[ONE, TWO]));
        peer.send(&json_message(
            json!({"cmd": "sars", "actID": 0, "cInf": "p:7", "iRes": 0, "a": []}),
        ));
        peer.expect(&gar(1, "aa", &[ONE]));
        peer.answer(1, 1);
        let second = peer.expect_at(&gar(2, "a1", &[ONE, TWO]));
        assert!(second - first < Duration::from_millis(1800));
        peer.answer(2, 1);
        peer.expect_logout();
    });
    wait_for_farm(&harness);
    let session = &harness.session;
    let unusable = |slot, target| match session.use_skill(slot, target).wait() {
        Err(ActionError::Unusable(why)) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(unusable("a3", None), "not enough mana for Meteor");
    assert_eq!(
        unusable("a4", None),
        "Ultimate is locked at this class rank"
    );
    assert_eq!(unusable("a9", None), "no skill in slot a9");
    assert!(matches!(
        session.kill(MonMapId(3)).wait(),
        Err(ActionError::NoTarget(why)) if why == "Frogzard #3 is in r2, not this cell"
    ));
    assert!(matches!(
        session.use_skill("a1", Some(MonMapId(1))).wait(),
        Err(ActionError::Refused(why)) if why == "the server refused Strike"
    ));
    session.use_skill("a1", Some(MonMapId(1))).wait().unwrap();
    session.logout();
    harness.finish();
}

#[test]
fn moves_where_the_respawn_says_and_aggroes_monsters_that_come_back() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        let state = |peer: &mut Peer, state: u8| {
            peer.send(&json_message(
                json!({"cmd": "uotls", "unm": "player1", "o": {"intState": state}}),
            ))
        };
        state(&mut peer, 0);
        peer.send("%xt%respawnMon%-1%1%");
        state(&mut peer, 1);
        peer.send("%xt%resTimed%-1%r2%Left%");
        peer.expect(&packet::move_to_cell(AreaId(43), "r2", "Left"));
        peer.send("%xt%resTimed%-1%");
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::aggro_mon(AreaId(43), &[MonMapId(1)]));
        peer.send("%xt%respawnMon%-1%2,1%");
        peer.expect(&packet::aggro_mon(AreaId(43), &[MonMapId(1)]));
        peer.send("%xt%multiLoginWarning%-1%");
    });
    let events = harness.finish();
    assert!(events.contains(&Event::Moved {
        cell: "r2".into(),
        pad: "Left".into(),
    }));
}

#[test]
fn leaving_combat_cancels_fights_queued_behind_another_action() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::transfer("player1", "yulgar"));
        thread::sleep(Duration::from_millis(300));
        peer.send(&move_to_area(43, "yulgar-2"));
        peer.send("%xt%server%-1%You joined \"yulgar-2\"!%");
        peer.expect(&packet::move_to_cell(AreaId(43), "Enter", "Spawn"));
        peer.expect(&packet::retrieve_user_datas(AreaId(43), &[EntityId(8)]));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    let joined = session.join("yulgar");
    let killing = session.kill(MonMapId(1));
    let leaving = session.exit_combat();
    leaving.wait().unwrap();
    assert!(matches!(killing.wait(), Err(ActionError::Interrupted)));
    joined.wait().unwrap();
    session.logout();
    harness.finish();
}

fn potions(world: &World) -> u32 {
    world
        .inventory
        .iter()
        .find(|i| i.id == ItemId(101))
        .map_or(0, |i| i.qty)
}

#[test]
fn loads_accepts_and_turns_in_quests_at_the_clients_pace() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let ids: Vec<QuestId> = (1..=15).map(QuestId).collect();
        let first = peer.expect_at(&packet::get_quests(AreaId(42), &ids[..14]));
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {}})));
        let second = peer.expect_at(&packet::get_quests(AreaId(42), &ids[14..]));
        assert!(second - first >= Limit::LoadQuests.gap() - SLACK);
        peer.send(&json_message(
            json!({"cmd": "getQuests", "quests": {"15": {"QuestID": 15, "sName": "Fifteen"}}}),
        ));
        let third = peer.expect_at(&packet::get_quests(AreaId(42), &[QuestId(30)]));
        assert!(third - second >= Limit::LoadQuests.gap() - SLACK);
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {"30": {
            "QuestID": 30, "sName": "Potion Run",
            "turnin": [{"ItemID": 101, "iQty": 2}],
            "oItems": {"101": {"ItemID": 101, "sName": "Health Potion"}},
            "reward": [
                {"ItemID": 600, "iQty": 1, "iRate": 100, "iType": 2},
                {"ItemID": 601, "iQty": 1, "iRate": 100, "iType": 2},
            ],
        }}})));
        let accepted = peer.expect_at(&packet::accept_quest(AreaId(42), QuestId(30)));
        peer.send(&json_message(
            json!({"cmd": "acceptQuest", "QuestID": 30, "bSuccess": 1, "msg": "success"}),
        ));
        let turn_in = packet::try_quest_complete(AreaId(42), QuestId(30), Some(ItemId(600)), 2);
        let turned_in = peer.expect_at(&turn_in);
        peer.send(&json_message(json!({"cmd": "turnIn", "sItems": "101:4"})));
        peer.send(&json_message(
            json!({"cmd": "ccqr", "QuestID": 30, "bSuccess": 1, "sName": "Potion Run"}),
        ));
        let again = peer.expect_at(&packet::accept_quest(AreaId(42), QuestId(30)));
        assert!(again - accepted >= Limit::AcceptQuest.gap() - SLACK);
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"101": {
            "CharItemID": 5001, "iQty": 2, "iQtyNow": 3,
        }}})));
        peer.send(&json_message(
            json!({"cmd": "acceptQuest", "QuestID": "30", "bSuccess": "1"}),
        ));
        let turn_in = packet::try_quest_complete(AreaId(42), QuestId(30), Some(ItemId(601)), 1);
        let retried = peer.expect_at(&turn_in);
        assert!(retried - turned_in >= Limit::TurnIn.gap() - SLACK);
        peer.send(&json_message(
            json!({"cmd": "ccqr", "QuestID": 30, "bSuccess": 0, "msg": "Missing Turn In Item"}),
        ));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    let ids: Vec<QuestId> = (1..=15).map(QuestId).collect();
    let loaded = session.load_quests(&ids).wait();
    assert!(matches!(loaded, Err(ActionError::NoTarget(_))));
    session.accept_quest(QuestId(30)).wait().unwrap();
    assert!(session.snapshot().accepted_quests.contains(&QuestId(30)));
    session.accept_quest(QuestId(30)).wait().unwrap();
    session
        .complete_quest(QuestId(30), Some(ItemId(600)), 2)
        .wait()
        .unwrap();
    let world = session.snapshot();
    assert!(!world.accepted_quests.contains(&QuestId(30)));
    assert_eq!(potions(&world), 1);
    session.accept_quest(QuestId(30)).wait().unwrap();
    assert_eq!(potions(&session.snapshot()), 3);
    let refused = session
        .complete_quest(QuestId(30), Some(ItemId(601)), 1)
        .wait()
        .unwrap_err();
    assert!(matches!(refused, ActionError::Refused(why) if why == "Missing Turn In Item"));
    assert!(session.snapshot().accepted_quests.contains(&QuestId(30)));
    session.logout();
    harness.finish();
}

#[test]
fn quest_actions_make_the_clients_checks_before_sending() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {
            "40": {"QuestID": 40, "sName": "Too High", "iLvl": 99},
            "41": {"QuestID": 41, "sName": "Choice",
                   "reward": [{"ItemID": 600, "iQty": 1, "iRate": 100, "iType": 2}]},
            "42": {"QuestID": 42, "sName": "Sword", "turnin": [{"ItemID": 100, "iQty": 1}]},
            "43": {"QuestID": 43, "sName": "Once", "bOnce": 1,
                   "turnin": [{"ItemID": 101, "iQty": 1}]},
            "44": {"QuestID": 44, "sName": "Echoed"},
        }})));
        for id in [41, 42, 43, 44] {
            peer.send(&json_message(
                json!({"cmd": "acceptQuest", "QuestID": id, "bSuccess": 1}),
            ));
        }
        let dropped = peer.expect_at(&packet::get_quests(AreaId(42), &[QuestId(45), QuestId(46)]));
        peer.send("%xt%warning%-1%Please slow down. Last action was too soon!%");
        let resent = peer.expect_at(&packet::get_quests(AreaId(42), &[QuestId(45), QuestId(46)]));
        assert!(resent - dropped >= Limit::LoadQuests.gap() + combat::BACKOFF_STEP - SLACK);
        peer.send(&json_message(json!({"cmd": "getQuests", "quests": {
            "45": {"QuestID": 45, "sName": "Real"},
        }})));
        peer.expect(&packet::try_quest_complete(
            AreaId(42),
            QuestId(44),
            None,
            1,
        ));
        peer.send("%xt%zm%tryQuestComplete%42%44%-1%false%1%wvz%");
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    while !session.snapshot().accepted_quests.contains(&QuestId(44)) {
        thread::sleep(Duration::from_millis(5));
    }
    let unusable = |pending: Pending| matches!(pending.wait(), Err(ActionError::Unusable(_)));
    assert!(unusable(session.accept_quest(QuestId(40))));
    assert!(unusable(session.complete_quest(QuestId(40), None, 1)));
    assert!(unusable(session.complete_quest(QuestId(41), None, 1)));
    assert!(unusable(session.complete_quest(
        QuestId(41),
        Some(ItemId(601)),
        1
    )));
    assert!(unusable(session.complete_quest(QuestId(42), None, 1)));
    assert!(unusable(session.complete_quest(QuestId(43), None, 2)));
    assert!(unusable(session.complete_quest(QuestId(43), None, 0)));
    let missing = session.load_quests(&[QuestId(46), QuestId(45), QuestId(44)]);
    assert!(matches!(missing.wait(), Err(ActionError::NoTarget(_))));
    assert!(session.snapshot().quests.contains_key(&QuestId(45)));
    let echoed = session.complete_quest(QuestId(44), None, 1).wait();
    assert!(matches!(echoed, Err(ActionError::Refused(_))));
    session.logout();
    harness.finish();
}

#[test]
fn picks_up_drops_while_the_bag_has_room() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let offer = |id: i64, name: &str, kind: &str, extra: Value| {
            let mut item =
                json!({"ItemID": id, "sName": name, "sType": kind, "iStk": 1, "iQty": 1});
            item.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            json_message(json!({"cmd": "dropItem", "items": {id.to_string(): item}}))
        };
        peer.send(&offer(1027, "Twilly Twig", "Staff", json!({})));
        peer.send(&offer(1028, "Spare Axe", "Axe", json!({})));
        peer.send(&offer(1029, "Gem", "Item", json!({"iStk": 10})));
        peer.send(&offer(1030, "Scroll", "Item", json!({"sMeta": " 12 "})));
        peer.send(&offer(1031, "Ore", "Resource", json!({})));
        peer.send(&offer(1032, "Warrior", "Class", json!({})));
        peer.expect(&packet::get_drop(AreaId(42), ItemId(1027)));
        peer.send(&json_message(json!({
            "cmd": "getDrop", "bSuccess": 1, "ItemID": 1027, "CharItemID": 7027,
            "iQty": 1, "iQtyNow": 1, "bBank": 0,
        })));
        peer.expect(&packet::get_drop(AreaId(42), ItemId(1029)));
        peer.send(&json_message(json!({
            "cmd": "getDrop", "bSuccess": 1, "ItemID": 1029, "CharItemID": 7029, "iQty": 1,
        })));
        peer.expect(&packet::get_drop(AreaId(42), ItemId(1032)));
        peer.send(&json_message(
            json!({"cmd": "getDrop", "bSuccess": 0, "ItemID": 1032}),
        ));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    while session.snapshot().drops.len() < 6 {
        thread::sleep(Duration::from_millis(5));
    }
    session.pick_drop(ItemId(1027)).wait().unwrap();
    let world = session.snapshot();
    assert!(world.inventory.iter().any(|i| i.id == ItemId(1027)));
    assert!(world.drops.iter().all(|d| d.id != ItemId(1027)));
    let full = |pending: Pending| matches!(pending.wait(), Err(ActionError::Unusable(_)));
    assert!(full(session.pick_drop(ItemId(1028))));
    assert!(full(session.pick_drop(ItemId(1030))));
    session.pick_drop(ItemId(1029)).wait().unwrap();
    let missing = session.pick_drop(ItemId(9999)).wait();
    assert!(matches!(missing, Err(ActionError::NoTarget(_))));
    assert!(full(session.pick_drop(ItemId(1031))));
    let refused = session.pick_drop(ItemId(1032)).wait();
    assert!(matches!(refused, Err(ActionError::Refused(_))));
    assert!(
        session
            .snapshot()
            .drops
            .iter()
            .any(|d| d.id == ItemId(1032))
    );
    session.logout();
    harness.finish();
}

#[test]
fn a_join_refuses_in_combat_and_when_dead_without_sending() {
    let (step, next) = mpsc::channel::<()>();
    let harness = Harness::start(Options::default(), move |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_farm();
        peer.send(&json_message(
            json!({"cmd": "cb", "p": {"player1": {"intState": 2}}}),
        ));
        next.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player1": {"intHP": 0, "intState": 0}}}),
        ));
        next.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "uotls", "unm": "player1", "o": {"intHP": 1000, "intState": 1}}),
        ));
        peer.expect(&packet::transfer("player1", "yulgar"));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    wait_until(session, |me| me.state == CombatState::InCombat);
    let in_combat = session.join("yulgar").wait();
    assert!(
        matches!(in_combat, Err(ActionError::InCombat)),
        "{in_combat:?}"
    );
    step.send(()).unwrap();
    wait_until(session, |me| me.state == CombatState::Dead);
    assert!(matches!(
        session.join("yulgar").wait(),
        Err(ActionError::Dead)
    ));
    step.send(()).unwrap();
    wait_until(session, |me| me.state == CombatState::Idle);
    let _joining = session.join("yulgar");
    thread::sleep(Duration::from_millis(100));
    session.logout();
    harness.finish();
}

fn shop_reply(id: i64, location: &str) -> String {
    json_message(json!({"cmd": "loadShop", "shopinfo": {
        "ShopID": id, "sName": format!("Shop {id}"), "bLimited": false, "Location": location,
        "items": [{"ItemID": 900, "ShopItemID": 4000 + id, "sName": "Sword", "iCost": "250",
                   "bCoins": 0, "iStk": 1, "iQty": 1, "FactionID": 1, "sType": "Sword"}],
    }}))
}

#[test]
fn loads_shops_once_each_and_refuses_one_known_to_open_elsewhere() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let first = peer.expect_at(&packet::load_shop(AreaId(42), ShopId(6)));
        peer.send(&shop_reply(6, "faraway"));
        let second = peer.expect_at(&packet::load_shop(AreaId(42), ShopId(5)));
        assert!(second - first >= Limit::LoadShop.gap() - SLACK);
        peer.send(&shop_reply(5, "yulgar, battleon"));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    session.load_shop(ShopId(6)).wait().unwrap();
    session.load_shop(ShopId(6)).wait().unwrap();
    session.load_shop(ShopId(5)).wait().unwrap();
    let off_map = session.load_shop(ShopId(6)).wait();
    assert!(
        matches!(off_map, Err(ActionError::Unusable(_))),
        "{off_map:?}"
    );
    session.load_shop(ShopId(5)).wait().unwrap();
    let shop = session.snapshot().shop.clone().unwrap();
    assert_eq!(
        (shop.id, shop.maps.clone()),
        (ShopId(5), vec!["yulgar".to_owned(), "battleon".to_owned()])
    );
    assert_eq!(
        shop.items[0],
        crate::ShopItem {
            listing: mira_protocol::ShopItemId(4005),
            id: ItemId(900),
            name: "Sword".into(),
            cost: 250,
            coins: false,
            member: false,
            max_stack: 1,
            qty: 1,
            faction: None,
            required_rep: 0,
            kind: "Sword".into(),
            slot: String::new(),
            meta: String::new(),
            link: String::new(),
            house: false,
            merge: vec![],
            token: 0,
            token_qty: 0,
            quest_slot: -1,
            quest_value: 0,
            enhancement_id: 0,
        }
    );
    session.logout();
    harness.finish();
}

#[test]
fn a_kick_reports_how_long_until_the_account_can_log_in() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.send("%xt%logoutWarning%-1%%30%");
        peer.send("<msg t='sys'><body action='logout' r='0'></body></msg>");
        peer.wait_for_close();
    });
    let events = harness.finish();
    assert_eq!(closed_with(&events), Some(Disconnect::Kicked { secs: 30 }));
}

#[test]
fn buys_and_sells_within_the_clients_limits() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::load_shop(AreaId(42), ShopId(7)));
        peer.send(&json_message(json!({"cmd": "loadShop", "shopinfo": {
            "ShopID": 7, "sName": "Mixed", "Location": "battleon", "items": [
                {"ItemID": 101, "ShopItemID": 70, "sName": "Health Potion", "iCost": 100,
                 "bCoins": 0, "iStk": 10, "iQty": 1, "sType": "Item"},
                {"ItemID": 902, "ShopItemID": 71, "sName": "Ore", "iCost": 0, "iStk": 50,
                 "iQty": 2, "sType": "Resource",
                 "turnin": [{"ItemID": 101, "sName": "Health Potion", "iQty": 3}]},
                {"ItemID": 903, "ShopItemID": 72, "sName": "Locked", "iCost": 1, "iStk": 1,
                 "iQty": 1, "iQSindex": 2, "iQSvalue": 11},
                {"ItemID": 904, "ShopItemID": 73, "sName": "Pricey", "iCost": 5000, "iStk": 1,
                 "iQty": 1},
                {"ItemID": 905, "ShopItemID": 74, "sName": "Token Buy", "iCost": 0, "iStk": 10,
                 "iQty": 1, "TokID": 101, "TokQty": 2},
            ],
        }})));
        let bought = peer.expect_at(&packet::buy_item(
            AreaId(42),
            ItemId(101),
            ShopId(7),
            ShopItemId(70),
            4,
        ));
        peer.send(&json_message(json!({
            "cmd": "buyItem", "bitSuccess": 1, "ItemID": 101, "CharItemID": 5001, "iQty": 4,
            "bBank": 0,
        })));
        let merged = peer.expect_at(&packet::buy_item(
            AreaId(42),
            ItemId(902),
            ShopId(7),
            ShopItemId(71),
            4,
        ));
        assert!(merged - bought >= Limit::BuyItem.gap() - SLACK);
        peer.send(&json_message(json!({
            "cmd": "buyItem", "bitSuccess": 0, "strMessage": "Sold out",
        })));
        peer.expect(&packet::sell_item(
            AreaId(42),
            ItemId(101),
            2,
            CharItemId(5001),
        ));
        peer.send(&json_message(json!({
            "cmd": "sellItem", "CharItemID": 5001, "iQty": 2, "iQtyNow": 7, "intAmount": 50,
            "bCoins": 0,
        })));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    let unusable = |pending: Pending| matches!(pending.wait(), Err(ActionError::Unusable(_)));
    assert!(unusable(session.buy(ShopItemId(70), 1)));
    session.load_shop(ShopId(7)).wait().unwrap();
    assert!(unusable(session.buy(ShopItemId(70), 6)));
    assert!(unusable(session.buy(ShopItemId(72), 1)));
    assert!(unusable(session.buy(ShopItemId(73), 1)));
    assert!(unusable(session.buy(ShopItemId(74), u32::MAX)));
    assert!(matches!(
        session.buy(ShopItemId(99), 1).wait(),
        Err(ActionError::NoTarget(_))
    ));
    session.buy(ShopItemId(70), 4).wait().unwrap();
    let world = session.snapshot();
    assert_eq!((potions(&world), world.me.as_ref().unwrap().gold), (9, 800));
    assert!(unusable(session.buy(ShopItemId(71), 7)));
    let refused = session.buy(ShopItemId(71), 4).wait();
    assert!(matches!(refused, Err(ActionError::Refused(why)) if why == "Sold out"));
    assert!(unusable(session.sell(ItemId(101), 10)));
    session.sell(ItemId(101), 2).wait().unwrap();
    let world = session.snapshot();
    assert_eq!((potions(&world), world.me.as_ref().unwrap().gold), (7, 850));
    session.logout();
    harness.finish();
}

fn serve_bank(items: Value) -> (String, JoinHandle<(Vec<String>, String)>) {
    use std::io::{BufRead, BufReader};
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", api.local_addr().unwrap());
    let http = thread::spawn(move || {
        let (mut stream, _) = api.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = Vec::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim().to_owned();
            if line.is_empty() {
                break;
            }
            if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                length = n.parse().unwrap();
            }
            head.push(line);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let bank = items.to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{bank}",
            bank.len()
        )
        .unwrap();
        (head, String::from_utf8(body).unwrap())
    });
    (base, http)
}

#[test]
fn loads_the_bank_once_over_http_with_the_character_id() {
    let (base, http) = serve_bank(json!([{"ItemID": 300, "CharItemID": 9300,
        "sName": "Banked Gem", "iQty": 4, "iStk": 10, "sType": "Item", "bBank": true}]));
    let harness = Harness::start_with_api(Options::default(), base, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    assert!(session.snapshot().bank.is_none());
    session.load_bank().wait().unwrap();
    session.load_bank().wait().unwrap();
    let (head, body) = http.join().unwrap();
    assert!(head[0].starts_with("POST /char/bank?v=0."), "{head:?}");
    assert!(head.contains(&"ccid: 4242".to_owned()), "{head:?}");
    assert!(head.contains(&format!("token: {TOKEN}")), "{head:?}");
    assert_eq!(body, "layout=%7B%22cat%22%3A%22all%22%7D");
    let world = session.snapshot();
    let bank = world.bank.as_ref().unwrap();
    assert_eq!((bank[0].id, bank[0].qty), (ItemId(300), 4));
    session.logout();
    harness.finish();
}

#[test]
fn moves_items_between_the_bank_and_inventory_after_the_clients_checks() {
    let banked = |id: i64, name: &str, kind: &str| {
        json!({"ItemID": id, "CharItemID": 9000 + id, "sName": name, "iQty": 1, "iStk": 1,
               "sType": kind, "bBank": true})
    };
    let (base, http) = serve_bank(json!([
        banked(301, "Banked Axe", "Axe"),
        banked(302, "Banked Bow", "Bow"),
    ]));
    let harness = Harness::start_with_api(Options::default(), base, |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::bank_from_inv(
            AreaId(42),
            ItemId(101),
            CharItemId(5001),
        ));
        peer.send(&json_message(
            json!({"cmd": "bankFromInv", "ItemID": 101, "bSuccess": 1}),
        ));
        peer.expect(&packet::bank_to_inv(
            AreaId(42),
            ItemId(301),
            CharItemId(9301),
        ));
        peer.send(&json_message(json!({"cmd": "bankToInv", "ItemID": "301"})));
        peer.expect(&packet::bank_swap_inv(
            AreaId(42),
            ItemId(301),
            CharItemId(9301),
            ItemId(302),
            CharItemId(9302),
        ));
        peer.send(&json_message(
            json!({"cmd": "bankSwapInv", "invItemID": 301, "bankItemID": "302"}),
        ));
        peer.expect(&packet::bank_from_inv(
            AreaId(42),
            ItemId(302),
            CharItemId(9302),
        ));
        peer.send(&json_message(json!({
            "cmd": "bankFromInv", "ItemID": 302, "bSuccess": 0, "msg": "Bank is full",
        })));
        peer.send(&json_message(
            json!({"cmd": "cb", "p": {"player1": {"intState": 2}}}),
        ));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    let unusable = |pending: Pending| matches!(pending.wait(), Err(ActionError::Unusable(_)));
    assert!(unusable(session.deposit(ItemId(100))));
    session.deposit(ItemId(101)).wait().unwrap();
    assert_eq!(session.snapshot().me.as_ref().unwrap().bank_count, 2);
    session.withdraw(ItemId(301)).wait().unwrap();
    assert!(unusable(session.withdraw(ItemId(302))));
    let world = session.snapshot();
    let held: Vec<ItemId> = world.inventory.iter().map(|i| i.id).collect();
    let stored: Vec<ItemId> = world.bank.as_ref().unwrap().iter().map(|i| i.id).collect();
    assert_eq!(
        (held, stored),
        (
            vec![ItemId(100), ItemId(301)],
            vec![ItemId(302), ItemId(101)]
        )
    );
    assert_eq!(world.me.as_ref().unwrap().bank_count, 1);
    session.swap(ItemId(301), ItemId(302)).wait().unwrap();
    let world = session.snapshot();
    assert!(world.inventory.iter().any(|i| i.id == ItemId(302)));
    assert!(
        world
            .bank
            .as_ref()
            .unwrap()
            .iter()
            .any(|i| i.id == ItemId(301))
    );
    let refused = session.deposit(ItemId(302)).wait();
    assert!(matches!(refused, Err(ActionError::Refused(why)) if why == "Bank is full"));
    wait_until(session, |me| me.state == CombatState::InCombat);
    let fighting = session.withdraw(ItemId(301)).wait();
    assert!(
        matches!(fighting, Err(ActionError::InCombat)),
        "{fighting:?}"
    );
    http.join().unwrap();
    session.logout();
    harness.finish();
}

#[test]
fn a_bank_load_interrupted_by_logout_reports_the_logout() {
    use std::io::{BufRead, BufReader};
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", api.local_addr().unwrap());
    let (logging_out, respond) = mpsc::channel::<()>();
    let (responded, answered) = mpsc::channel::<()>();
    let (requested, asked) = mpsc::channel::<()>();
    let http = thread::spawn(move || {
        let (mut stream, _) = api.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 2 {
            line.clear();
        }
        requested.send(()).unwrap();
        respond.recv().unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n[]")
            .unwrap();
        responded.send(()).unwrap();
    });
    let harness = Harness::start_with_api(Options::default(), base, move |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        peer.expect(&packet::logout());
        logging_out.send(()).unwrap();
        answered.recv().unwrap();
        thread::sleep(Duration::from_millis(100));
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    let loading = session.load_bank();
    asked.recv_timeout(Duration::from_secs(5)).unwrap();
    session.logout();
    let result = loading.wait();
    assert!(
        matches!(result, Err(ActionError::Closed(Disconnect::LoggedOut))),
        "{result:?}"
    );
    http.join().unwrap();
    harness.finish();
}

#[test]
fn equips_gear_after_the_clients_checks_and_waits_out_each_lock() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        let gear = |id: i64, name: &str, slot: &str, extra: Value| {
            let mut item = json!({"ItemID": id, "CharItemID": 8000 + id, "sName": name,
                "iQty": 1, "iStk": 1, "sType": "Gear", "sES": slot, "EnhID": 5, "EnhLvl": 1});
            item.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            item
        };
        peer.serve_with_items(
            "5.31",
            json!([
                gear(1, "Old Sword", "Weapon", json!({"bEquip": 1})),
                gear(2, "New Sword", "Weapon", json!({"EnhLvl": 10})),
                gear(3, "Helm", "he", json!({"bEquip": 1})),
                gear(4, "Bare Cape", "ba", json!({"EnhID": 0})),
                gear(5, "Big Sword", "Weapon", json!({"EnhLvl": 99})),
                gear(6, "Pet", "pe", json!({"EnhID": 0})),
                gear(7, "Member Helm", "he", json!({"bUpg": 1})),
            ]),
        );
        let worn = |cmd: &str, id: i64| json_message(json!({"cmd": cmd, "uid": 7, "ItemID": id}));
        let first = peer.expect_at(&packet::equip_item(AreaId(42), ItemId(2)));
        peer.send(&worn("equipItem", 2));
        peer.expect(&packet::unequip_item(AreaId(42), ItemId(3)));
        peer.send(&worn("unequipItem", 3));
        let second = peer.expect_at(&packet::equip_item(AreaId(42), ItemId(6)));
        assert!(second - first >= Limit::EquipItem.gap() - SLACK);
        peer.send(&worn("equipItem", 6));
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"8": {
            "ItemID": 8, "CharItemID": 8008, "sName": "Given Sword", "iQty": 1, "iStk": 1,
            "sType": "Sword", "sES": "Weapon", "EnhID": 5, "EnhLvl": 1,
        }}})));
        peer.expect(&packet::equip_item(AreaId(42), ItemId(8)));
        peer.send(&worn("equipItem", 8));
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {"9": {
            "ItemID": 9, "sName": "Dropped Class", "iQty": 1, "iStk": 1, "sType": "Class",
            "sES": "ar",
        }}})));
        peer.expect(&packet::get_drop(AreaId(42), ItemId(9)));
        peer.send(&json_message(json!({
            "cmd": "getDrop", "bSuccess": 1, "ItemID": 9, "CharItemID": 8009, "iQty": 1,
            "EnhID": 5, "EnhLvl": 1,
        })));
        peer.expect(&packet::equip_item(AreaId(42), ItemId(9)));
        peer.send(&worn("equipItem", 9));
        peer.send(&json_message(
            json!({"cmd": "cb", "p": {"player1": {"intState": 2}}}),
        ));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    let unusable = |pending: Pending| matches!(pending.wait(), Err(ActionError::Unusable(_)));
    assert!(unusable(session.equip(ItemId(5))));
    assert!(unusable(session.equip(ItemId(4))));
    assert!(unusable(session.equip(ItemId(7))));
    assert!(unusable(session.unequip(ItemId(1))));
    session.equip(ItemId(1)).wait().unwrap();
    session.equip(ItemId(2)).wait().unwrap();
    session.unequip(ItemId(3)).wait().unwrap();
    session.equip(ItemId(6)).wait().unwrap();
    let world = session.snapshot();
    let worn: Vec<ItemId> = world
        .inventory
        .iter()
        .filter(|i| i.equipped)
        .map(|i| i.id)
        .collect();
    assert_eq!(worn, [ItemId(2), ItemId(6)]);
    while !session
        .snapshot()
        .inventory
        .iter()
        .any(|i| i.id == ItemId(8))
    {
        thread::sleep(Duration::from_millis(5));
    }
    session.equip(ItemId(8)).wait().unwrap();
    while session.snapshot().drops.is_empty() {
        thread::sleep(Duration::from_millis(5));
    }
    session.pick_drop(ItemId(9)).wait().unwrap();
    session.equip(ItemId(9)).wait().unwrap();
    wait_until(session, |me| me.state == CombatState::InCombat);
    assert!(matches!(
        session.equip(ItemId(3)).wait(),
        Err(ActionError::InCombat)
    ));
    session.logout();
    harness.finish();
}

#[test]
fn names_enhancements_on_gear_with_the_servers_proc_names() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        let item = |id: i64, slot: &str, extra: Value| {
            let mut item = json!({"ItemID": id, "CharItemID": 8000 + id, "sName": "Thing",
                "iQty": 1, "iStk": 1, "sType": "Gear", "sES": slot, "EnhID": 5, "EnhLvl": 1,
                "EnhPatternID": 9, "iRty": 12});
            item.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            item
        };
        peer.serve_with_items(
            "5.31",
            json!([
                item(1, "Weapon", json!({"ProcID": 2, "EnhRty": 3})),
                item(2, "pe", json!({})),
                item(3, "ba", json!({"EnhPatternID": 11})),
                item(4, "he", json!({"EnhID": 0})),
            ]),
        );
        peer.send(&json_message(json!({
            "cmd": "sAct", "actions": {"active": []},
            "procs": {"2": {"nam": "Spiral Carve II", "pct": 0.1}},
        })));
        peer.send(&json_message(json!({"cmd": "dropItem", "items": {"20": {
            "ItemID": 20, "sName": "Dropped Axe", "iQty": 1, "iStk": 1, "sType": "Axe",
            "sES": "Weapon", "EnhID": 7, "EnhLvl": 3, "EnhPatternID": 2,
        }}})));
        peer.send("%xt%server%-1%sent%");
        peer.expect(&packet::logout());
    });
    harness
        .events
        .iter()
        .find(|e| matches!(e, Event::Notice { text, .. } if text == "sent"))
        .unwrap();
    let world = harness.session.snapshot();
    let offered = serde_json::to_value(&world.drops[0]).unwrap();
    assert_eq!(
        offered["enhancement"],
        json!({"id": 7, "level": 3, "pattern": 2, "proc_id": 0, "plus": 0})
    );
    let names: Vec<Option<String>> = world
        .inventory
        .iter()
        .map(|item| world.enhancement_name(item))
        .collect();
    assert_eq!(
        names,
        [
            Some("Lucky +2, Spiral Carve II".to_owned()),
            None,
            Some("Forge, Absolution".to_owned()),
            None,
        ]
    );
    harness.session.logout();
    harness.finish();
}

#[test]
fn fills_the_item_slot_and_uses_it_at_the_clients_pace() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_with_items(
            "5.31",
            json!([
                {"ItemID": 101, "CharItemID": 5001, "sName": "Health Potion", "iQty": 5,
                 "iStk": 10, "sType": "Item", "sES": "None", "sLink": "potion", "sMeta": "1044"},
                {"ItemID": 102, "CharItemID": 5002, "sName": "Gem", "iQty": 1, "iStk": 10,
                 "sType": "Item", "sES": "None", "sLink": "None"},
            ]),
        );
        peer.send(&json_message(json!({"cmd": "sAct", "actions": {"active": [
            {"ref": "aa", "nam": "Auto Attack", "cd": 2000, "tgt": "h"},
            {"ref": "i1", "nam": "Potions", "cd": 60000, "tgt": "f", "mp": 0},
        ]}})));
        let slot = |cd: u32| {
            json_message(json!({"cmd": "seia", "iRes": 1, "o": {
                "id": 1044, "cd": cd, "tgt": "s", "mp": 0, "anim": "Salute", "strl": "",
            }}))
        };
        peer.expect(&packet::geia(AreaId(42), "potion", "1044", ItemId(101)));
        peer.send(&slot(2500));
        let me = [Aim::Player(EntityId(7))];
        let used = peer.expect_at(&packet::gar_item(0, "i1", &me, ItemId(101)));
        peer.answer(0, 1);
        peer.expect(&packet::geia(AreaId(42), "potion", "1044", ItemId(101)));
        peer.send(&slot(200));
        let again = peer.expect_at(&packet::gar_item(1, "i1", &me, ItemId(101)));
        assert!(again - used >= Duration::from_millis(2375) - SLACK);
        peer.answer(1, 1);
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    wait_until(session, |me| me.skills.iter().any(|s| s.slot == "i1"));
    let unusable = |pending: Pending| matches!(pending.wait(), Err(ActionError::Unusable(_)));
    assert!(unusable(session.use_skill("i1", None)));
    assert!(unusable(session.equip(ItemId(102))));
    session.equip(ItemId(101)).wait().unwrap();
    let world = session.snapshot();
    let me = world.me.as_ref().unwrap();
    assert_eq!(me.item_slot, Some(ItemId(101)));
    let slot = me.skills.iter().find(|s| s.slot == "i1").unwrap();
    assert_eq!((slot.cooldown_ms, slot.target), (2500, SkillTarget::Myself));
    assert!(
        world
            .inventory
            .iter()
            .any(|i| i.id == ItemId(101) && i.equipped)
    );
    session.use_skill("i1", None).wait().unwrap();
    session.unequip(ItemId(101)).wait().unwrap();
    assert_eq!(session.snapshot().me.as_ref().unwrap().item_slot, None);
    assert!(unusable(session.use_skill("i1", None)));
    session.equip(ItemId(101)).wait().unwrap();
    session.use_skill("i1", None).wait().unwrap();
    session.logout();
    harness.finish();
}

#[test]
fn the_item_slot_keeps_its_fill_across_class_changes_and_restarts_its_clock() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_with_items(
            "5.31",
            json!([{"ItemID": 101, "CharItemID": 5001, "sName": "Health Potion", "iQty": 1,
                    "iStk": 10, "sType": "Item", "sES": "None", "sLink": "potion",
                    "sMeta": "1044"}]),
        );
        let skills = json_message(json!({"cmd": "sAct", "actions": {"active": [
            {"ref": "aa", "nam": "Auto Attack", "cd": 2000, "tgt": "h"},
            {"ref": "i1", "nam": "Potions", "cd": 60000, "tgt": "f", "mp": 0, "tgtMax": 2},
        ]}}));
        peer.send(&skills);
        let slot =
            |cd: u32| json_message(json!({"cmd": "seia", "iRes": 1, "o": {"cd": cd, "tgt": "s"}}));
        let me = [Aim::Player(EntityId(7))];
        peer.expect(&packet::geia(AreaId(42), "potion", "1044", ItemId(101)));
        peer.send(&slot(300));
        peer.expect(&packet::gar_item(0, "i1", &me, ItemId(101)));
        peer.answer(0, 1);
        peer.expect(&packet::geia(AreaId(42), "potion", "1044", ItemId(101)));
        let refilled = Instant::now();
        peer.send(&slot(60000));
        peer.expect(&packet::gar_item(1, "i1", &me, ItemId(101)));
        assert!(refilled.elapsed() < Duration::from_secs(2));
        peer.answer(1, 1);
        peer.send(&skills);
        peer.expect(&packet::gar_item(2, "i1", &me, ItemId(101)));
        peer.answer(2, 1);
        peer.send(&json_message(json!({"cmd": "turnIn", "sItems": "101:1"})));
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"101": {
            "ItemID": 101, "CharItemID": 5009, "sName": "Health Potion", "iQty": 1,
            "iStk": 10, "sType": "Item", "sES": "None", "sLink": "potion", "sMeta": "1044",
        }}})));
        peer.send("%xt%server%-1%restocked%");
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    wait_until(session, |me| me.skills.iter().any(|s| s.slot == "i1"));
    session.equip(ItemId(101)).wait().unwrap();
    session.use_skill("i1", None).wait().unwrap();
    thread::sleep(Duration::from_millis(1600));
    session.unequip(ItemId(101)).wait().unwrap();
    session.equip(ItemId(101)).wait().unwrap();
    session.use_skill("i1", None).wait().unwrap();
    thread::sleep(Duration::from_millis(1600));
    let used = session.use_skill("i1", None);
    used.wait().unwrap();
    harness
        .events
        .iter()
        .find(|e| matches!(e, Event::Notice { text, .. } if text == "restocked"))
        .unwrap();
    session.unequip(ItemId(101)).wait().unwrap();
    assert_eq!(session.snapshot().me.as_ref().unwrap().item_slot, None);
    let empty = session.use_skill("i1", None).wait();
    assert!(matches!(empty, Err(ActionError::Unusable(_))), "{empty:?}");
    session.logout();
    harness.finish();
}

#[test]
fn picks_up_map_items_one_per_second() {
    let harness = Harness::start(Options::default(), |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let first = peer.expect_at(&packet::get_map_item(AreaId(42), MapItemId(56)));
        peer.send(&json_message(json!({"cmd": "addItems", "items": {"700": {
            "ItemID": 700, "sName": "Tavern Token", "iQty": 1, "iStk": 7, "bTemp": true,
        }}})));
        let second = peer.expect_at(&packet::get_map_item(AreaId(42), MapItemId(56)));
        assert!(second - first >= Limit::MapItem.gap() - SLACK);
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    session.get_map_item(MapItemId(56)).wait().unwrap();
    session.get_map_item(MapItemId(56)).wait().unwrap();
    while !session.snapshot().temp.iter().any(|i| i.id == ItemId(700)) {
        thread::sleep(Duration::from_millis(5));
    }
    session.logout();
    harness.finish();
}

#[test]
fn rests_until_full_at_the_clients_pace() {
    let (rested, next) = mpsc::channel::<()>();
    let harness = Harness::start(Options::default(), move |listener| {
        let mut peer = Peer::accept(&listener);
        peer.serve_to_ready("5.31");
        let first = peer.expect_at(&packet::rest_request());
        peer.expect(&packet::emote("rest"));
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player1": {"intHP": 1000}}}),
        ));
        next.recv().unwrap();
        peer.send(&json_message(
            json!({"cmd": "ct", "p": {"player1": {"intHP": 400}}}),
        ));
        let second = peer.expect_at(&packet::rest_request());
        assert!(second - first >= Limit::Rest.gap() - SLACK);
        peer.expect(&packet::emote("rest"));
        peer.send(&json_message(
            json!({"cmd": "cb", "p": {"player1": {"intState": 2}}}),
        ));
        peer.expect(&packet::logout());
    });
    let session = &harness.session;
    session.ready().wait().unwrap();
    session.rest().wait().unwrap();
    assert_eq!(session.snapshot().me.as_ref().unwrap().hp, 1000);
    session.rest().wait().unwrap();
    rested.send(()).unwrap();
    wait_until(session, |me| me.hp == 400);
    let interrupted = session.rest().wait();
    assert!(
        matches!(interrupted, Err(ActionError::InCombat)),
        "{interrupted:?}"
    );
    session.logout();
    harness.finish();
}
