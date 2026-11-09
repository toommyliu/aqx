use std::io::{self, BufRead, BufReader, Read, Write};
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

    pub fn accept_within(listener: &TcpListener, wait: Duration) -> Option<Peer> {
        listener.set_nonblocking(true).unwrap();
        let until = Instant::now() + wait;
        let accepted = loop {
            match listener.accept() {
                Ok((stream, _)) => break Some(stream),
                Err(_) if Instant::now() < until => thread::sleep(Duration::from_millis(5)),
                Err(_) => break None,
            }
        };
        listener.set_nonblocking(false).unwrap();
        let stream = accepted?;
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Some(Peer {
            stream,
            buf: Vec::new(),
        })
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

pub struct MapCell<'a> {
    pub name: &'a str,
    pub pads: &'a [(&'a str, i32, i32)],
    pub doors: &'a [(i64, &'a str, &'a str)],
}

impl<'a> MapCell<'a> {
    pub fn named(name: &'a str) -> Self {
        MapCell {
            name,
            pads: &[],
            doors: &[],
        }
    }
}

pub fn map_swf(cells: &[&str]) -> Vec<u8> {
    let cells: Vec<_> = cells.iter().map(|name| MapCell::named(name)).collect();
    map_swf_with(&cells)
}

pub fn map_swf_with(cells: &[MapCell]) -> Vec<u8> {
    fn tag(body: &mut Vec<u8>, kind: u16, data: &[u8]) {
        if data.len() < 63 {
            body.extend_from_slice(&((kind << 6) | data.len() as u16).to_le_bytes());
        } else {
            body.extend_from_slice(&((kind << 6) | 63).to_le_bytes());
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        }
        body.extend_from_slice(data);
    }
    let mut body = vec![0, 0, 24, 1, 0];
    let mut classes = vec![2, 0];
    for (id, class) in [(0u16, "map_fla.MainTimeline"), (1, "map_fla.Pad_1")] {
        classes.extend_from_slice(&id.to_le_bytes());
        classes.extend_from_slice(class.as_bytes());
        classes.push(0);
    }
    tag(&mut body, 76, &classes);
    let abc = [&[1, 0, 0, 0, 0][..], &door_setters(cells)].concat();
    tag(&mut body, 82, &abc);
    for cell in cells {
        tag(&mut body, 43, &[cell.name.as_bytes(), &[0]].concat());
        for (depth, (name, x, y)) in (1u16..).zip(cell.pads) {
            let mut place = vec![2 | 4 | 32];
            place.extend_from_slice(&depth.to_le_bytes());
            place.extend_from_slice(&1u16.to_le_bytes());
            let mut bits = vec![false, false];
            for (value, width) in [(20, 5), (x * 20, 20), (y * 20, 20)] {
                bits.extend((0..width).rev().map(|i| (value >> i) & 1 == 1));
            }
            place.extend(bits.chunks(8).map(|byte| {
                (0..8).fold(0u8, |acc, i| {
                    acc << 1 | u8::from(byte.get(i) == Some(&true))
                })
            }));
            place.extend_from_slice(name.as_bytes());
            place.push(0);
            tag(&mut body, 26, &place);
        }
        tag(&mut body, 1, &[]);
        for depth in (1u16..).take(cell.pads.len()) {
            tag(&mut body, 28, &depth.to_le_bytes());
        }
    }
    body.extend_from_slice(&[0, 0]);
    [
        b"FWS\x0a".as_slice(),
        &((body.len() + 8) as u32).to_le_bytes(),
        &body,
    ]
    .concat()
}

fn door_setters(cells: &[MapCell]) -> Vec<u8> {
    fn u30(out: &mut Vec<u8>, mut n: usize) {
        loop {
            let byte = (n & 127) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 128);
        }
    }
    let doors: Vec<_> = (1..)
        .zip(cells)
        .flat_map(|(frame, cell)| cell.doors.iter().map(move |door| (frame, door)))
        .collect();
    let mut strings = vec!["map_fla", "MainTimeline", "tCell", "tPad", "tID"];
    let mut names = Vec::new();
    for (n, (frame, (_, cell, pad))) in doors.iter().enumerate() {
        names.push((
            format!("door{n}"),
            format!("__setProp_door{n}_Scene1_pads_{frame}"),
            *cell,
            *pad,
        ));
    }
    for (instance, setter, cell, pad) in &names {
        strings.extend([instance.as_str(), setter.as_str(), cell, pad]);
    }
    let string = |s: &str| strings.iter().position(|t| *t == s).unwrap() + 1;
    let mut multinames: Vec<&str> = vec!["MainTimeline", "tCell", "tPad", "tID"];
    for (instance, setter, _, _) in &names {
        multinames.extend([instance.as_str(), setter.as_str()]);
    }
    let multiname = |s: &str| multinames.iter().position(|t| *t == s).unwrap() + 1;
    let mut abc = vec![16, 0, 46, 0];
    u30(&mut abc, doors.len() + 1);
    for (_, (id, _, _)) in &doors {
        u30(&mut abc, *id as u32 as usize);
    }
    abc.extend([0, 0]);
    u30(&mut abc, strings.len() + 1);
    for s in &strings {
        u30(&mut abc, s.len());
        abc.extend_from_slice(s.as_bytes());
    }
    abc.extend([2, 0x16, 1, 0]);
    u30(&mut abc, multinames.len() + 1);
    for name in &multinames {
        abc.push(7);
        u30(&mut abc, 1);
        u30(&mut abc, string(name));
    }
    u30(&mut abc, doors.len() + 1);
    for _ in 0..=doors.len() {
        abc.extend([0, 0, 0, 0]);
    }
    abc.extend([0, 1]);
    u30(&mut abc, multiname("MainTimeline"));
    abc.extend([0, 0, 0, 0]);
    u30(&mut abc, names.len());
    for (method, (_, setter, _, _)) in (1..).zip(&names) {
        u30(&mut abc, multiname(setter));
        abc.extend([1, 0]);
        u30(&mut abc, method);
    }
    abc.extend([0, 0, 1, 0, 0]);
    u30(&mut abc, names.len());
    for (n, (instance, _, cell, pad)) in names.iter().enumerate() {
        u30(&mut abc, n + 1);
        abc.extend([2, 1, 0, 1]);
        let mut code = vec![0xd0, 0x30];
        for (op, operand, property) in [
            (0x2c, string(cell), "tCell"),
            (0x2c, string(pad), "tPad"),
            (0x2d, n + 1, "tID"),
        ] {
            code.push(0x60);
            u30(&mut code, multiname(instance));
            code.push(op);
            u30(&mut code, operand);
            code.push(0x61);
            u30(&mut code, multiname(property));
        }
        code.push(0x47);
        u30(&mut abc, code.len());
        abc.extend(code);
        abc.extend([0, 0]);
    }
    abc
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

pub fn read_request(stream: &TcpStream) -> (Vec<String>, String) {
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
    (head, String::from_utf8(body).unwrap())
}

pub fn serve_api(
    replies: mpsc::Receiver<String>,
    stop: mpsc::Receiver<()>,
) -> (String, JoinHandle<Vec<(Instant, String)>>) {
    let api = TcpListener::bind("127.0.0.1:0").unwrap();
    api.set_nonblocking(true).unwrap();
    let base = format!("http://{}", api.local_addr().unwrap());
    let http = thread::spawn(move || {
        let mut requests = Vec::new();
        while matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            let Ok((mut stream, _)) = api.accept() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            let (_, body) = read_request(&stream);
            requests.push((Instant::now(), body));
            let reply = replies
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_default();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
        }
        requests
    });
    (base, http)
}

pub fn login_reply(token: &str, port: u16) -> String {
    json!({
        "login": {"bSuccess": 1, "sToken": token, "iUpg": 0},
        "servers": [{"sName": "Test", "sIP": "127.0.0.1", "iPort": port, "bOnline": 1}],
    })
    .to_string()
}
