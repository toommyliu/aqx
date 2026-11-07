use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use mira_protocol::{AreaId, EntityId, MonMapId, packet};
use serde_json::{Value, json};

use crate::api::{Account, DeviceId, Server, Token};
use crate::{DEFAULT_CLIENT_VERSION, Event, Handle, Options, spawn};

pub const TOKEN: &str = "tok-abc123";
pub const API_OK: &str = "<msg t='sys'><body action='apiOK' r='0'></body></msg>";
pub const JOIN_OK: &str =
    "<msg t='sys'><body action='joinOK' r='0'><uLs r='42'></uLs></body></msg>";
pub const FARM_SKILLS: &str = r#"{"cmd": "sAct", "actions": {"active": [
    {"ref": "aa", "nam": "Auto Attack", "cd": 2000, "mp": -10, "tgt": "h", "tgtMax": 1},
    {"ref": "a1", "nam": "Strike", "cd": 4000, "mp": 10, "tgt": "h", "tgtMax": 2},
    {"ref": "a2", "nam": "Shield", "cd": 4000, "mp": 5, "tgt": "s", "tgtMax": 1},
    {"ref": "a3", "nam": "Meteor", "cd": 4000, "mp": 999, "tgt": "h", "tgtMax": 1},
    {"ref": "a4", "nam": "Ultimate", "cd": 4000, "mp": 0, "tgt": "h", "tgtMax": 1, "isOK": false}
]}}"#;
pub const LOGIN_OK: &str = "%xt%loginResponse%-1%true%7%player1%Welcome%0%sVersion=R0048%5.31%";

pub struct Peer {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Peer {
    pub fn accept(listener: &TcpListener) -> Peer {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Peer {
            stream,
            buf: Vec::new(),
        }
    }

    pub fn set_read_timeout(&self, timeout: Duration) {
        self.stream.set_read_timeout(Some(timeout)).unwrap();
    }

    fn take_frame(&mut self) -> Option<String> {
        let end = self.buf.iter().position(|&b| b == 0)?;
        let frame: Vec<u8> = self.buf.drain(..=end).collect();
        Some(String::from_utf8(frame[..end].to_vec()).unwrap())
    }

    pub fn recv(&mut self) -> String {
        loop {
            if let Some(frame) = self.take_frame() {
                return frame;
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk).unwrap();
            assert!(n > 0, "the client closed the connection");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    pub fn recv_for(&mut self, window: Duration) -> Vec<String> {
        let until = Instant::now() + window;
        let mut frames = Vec::new();
        loop {
            while let Some(frame) = self.take_frame() {
                frames.push(frame);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            self.stream.set_read_timeout(Some(left)).unwrap();
            let mut chunk = [0u8; 4096];
            match self.stream.read(&mut chunk) {
                Ok(n) => {
                    assert!(n > 0, "the client closed the connection");
                    self.buf.extend_from_slice(&chunk[..n]);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(e) => panic!("{e}"),
            }
        }
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        frames
    }

    pub fn expect(&mut self, packet: &str) {
        assert_eq!(self.recv(), packet);
    }

    pub fn send(&mut self, text: &str) {
        self.send_bytes(text.as_bytes());
    }

    pub fn send_bytes(&mut self, frame: &[u8]) {
        self.stream.write_all(frame).unwrap();
        self.stream.write_all(&[0]).unwrap();
    }

    pub fn expect_silence(&mut self, quiet: Duration) {
        assert!(!self.buf.contains(&0), "the client sent something early");
        self.stream.set_read_timeout(Some(quiet)).unwrap();
        let mut chunk = [0u8; 4096];
        let read = self.stream.read(&mut chunk);
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        match read {
            Ok(n) => panic!(
                "the client sent {:?} early",
                String::from_utf8_lossy(&chunk[..n])
            ),
            Err(e) => assert!(
                matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ),
                "{e}"
            ),
        }
    }

    pub fn send_together(&mut self, texts: &[&str]) {
        let mut frames = Vec::new();
        for text in texts {
            frames.extend_from_slice(text.as_bytes());
            frames.push(0);
        }
        self.stream.write_all(&frames).unwrap();
    }

    pub fn expect_logout(&mut self) {
        while self.recv() != packet::logout() {}
    }

    pub fn wait_for_close(&mut self) {
        let mut chunk = [0u8; 4096];
        while self.stream.read(&mut chunk).unwrap() > 0 {}
    }

    pub fn serve_to_ready(&mut self) {
        self.serve_to_ready_with_token(DEFAULT_CLIENT_VERSION, TOKEN);
    }

    pub fn serve_to_ready_with_token(&mut self, version: &str, token: &str) {
        let items = json!([
            {"ItemID": 100, "CharItemID": 5000, "sName": "Default Sword", "iQty": 1, "iStk": 1,
             "sType": "Sword", "sES": "Weapon", "bEquip": 1},
            {"ItemID": "101", "CharItemID": "5001", "sName": "Health Potion", "iQty": "5",
             "iStk": 10, "sType": "Item", "sES": "None", "bEquip": false},
        ]);
        self.serve(version, token, items, json!([]));
    }

    pub fn serve_with_items(&mut self, items: Value) {
        self.serve(DEFAULT_CLIENT_VERSION, TOKEN, items, json!([]));
    }

    pub fn serve_with_inventory(&mut self, items: Value, hitems: Value) {
        self.serve(DEFAULT_CLIENT_VERSION, TOKEN, items, hitems);
    }

    pub fn answer(&mut self, act: u8, res: u8) {
        self.send(&json_message(json!({
            "cmd": "ct",
            "sarsa": [{"actID": act, "cInf": "p:7", "iRes": res, "a": []}],
        })));
    }

    pub fn serve_farm(&mut self) {
        self.serve_to_ready();
        self.send(&json_message(serde_json::from_str(FARM_SKILLS).unwrap()));
        self.send(&json_message(
            json!({"cmd": "stu", "sta": {"$tha": 0.84, "$cmc": 1}}),
        ));
        self.join_farm();
    }

    pub fn join_farm(&mut self) {
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

    pub fn serve(&mut self, version: &str, token: &str, items: Value, hitems: Value) {
        self.expect(&packet::version_check());
        self.send(API_OK);
        self.expect(&packet::login("player1", version, token));
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
                "CharID": "4242", "iBankSlots": 3, "iHouseSlots": 1,
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
            json!({"cmd": "loadInventoryBig", "items": items, "hitems": hitems, "bankCount": 1, "factions": [
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

pub fn device_id() -> DeviceId {
    DeviceId::for_tests()
}

pub fn json_message(o: Value) -> String {
    json!({"t": "xt", "b": {"r": -1, "o": o}}).to_string()
}

pub fn map_swf(cells: &[&str]) -> Vec<u8> {
    let mut body = vec![0, 0, 24, 1, 0];
    for cell in cells {
        body.extend_from_slice(&((43 << 6) | (cell.len() + 1) as u16).to_le_bytes());
        body.extend_from_slice(cell.as_bytes());
        body.push(0);
        body.extend_from_slice(&(1u16 << 6).to_le_bytes());
    }
    body.extend_from_slice(&[0, 0]);
    [
        b"FWS\x0a".as_slice(),
        &((body.len() + 8) as u32).to_le_bytes(),
        &body,
    ]
    .concat()
}

pub fn move_to_area(area: i64, name: &str) -> String {
    json_message(json!({
        "cmd": "moveToArea",
        "areaId": area,
        "areaName": name,
        "uoBranch": [
            {"entID": 7, "strUsername": "player1", "strFrame": "Enter", "intHP": 950, "intHPMax": 1000},
            {"entID": 8, "strUsername": "player2", "strFrame": "Enter"},
            {"entID": 9, "strUsername": "player3", "strFrame": "r2", "intLevel": 10, "afk": true},
        ],
    }))
}

pub fn compress(text: &str) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap());
    [b"Z".as_slice(), encoded.as_bytes()].concat()
}

pub struct Harness {
    pub session: Handle,
    pub events: mpsc::Receiver<Event>,
    script: JoinHandle<()>,
}

impl Harness {
    pub fn start(options: Options, script: impl FnOnce(TcpListener) + Send + 'static) -> Harness {
        Harness::start_with_api(options, "http://127.0.0.1:9".into(), script)
    }

    pub fn start_with_api(
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
            token: Token::for_tests(TOKEN),
            member: false,
            servers: vec![server.clone()],
            device: DeviceId::for_tests(),
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

    pub fn finish(self) -> Vec<Event> {
        let events = self.events.iter().collect();
        self.script.join().expect("the fake server failed");
        events
    }
}
