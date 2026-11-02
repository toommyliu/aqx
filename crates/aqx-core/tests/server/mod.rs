#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aqx_core::{Control, Login, Phase, Server, Session};

pub const FARM: &str = r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
    "mondef":[{"MonID":10,"strMonName":"Mosquito"},{"MonID":12,"strMonName":"Treeant"}],
    "monmap":[{"MonMapID":1,"MonID":10,"strFrame":"r1"},{"MonMapID":2,"MonID":12,"strFrame":"r2"}],
    "monBranch":[{"MonMapID":1,"intHP":540,"intHPMax":540,"intState":1},{"MonMapID":2,"intHP":900,"intHPMax":900,"intState":1}],
    "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}]}"#;

const SKILLS: &str = r#"{"cmd":"sAct","actions":{"active":[
    {"ref":"aa","nam":"Auto Attack","cd":2000,"mp":-10,"tgt":"h","tgtMax":1},
    {"ref":"a1","nam":"Slash","cd":3000,"mp":10,"tgt":"h","tgtMax":3},
    {"ref":"a2","nam":"Guard","cd":3000,"mp":10,"tgt":"s","tgtMax":1},
    {"ref":"a3","nam":"Cleave","cd":6000,"mp":20,"tgt":"h","tgtMax":1},
    {"ref":"a4","nam":"Finisher","cd":12000,"mp":30,"tgt":"h","tgtMax":1}]}}"#;

pub fn json(o: &str) -> String {
    format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#)
}

pub struct Wire {
    sock: TcpStream,
    seen: Sender<(Instant, String)>,
}

impl Wire {
    pub fn send(&mut self, msg: &str) {
        self.sock.write_all(msg.as_bytes()).unwrap();
        self.sock.write_all(b"\0").unwrap();
    }

    pub fn object(&mut self, o: &str) {
        self.send(&json(o));
    }

    pub fn mark(&self, marker: &str) {
        let _ = self.seen.send((Instant::now(), marker.into()));
    }

    pub fn later(&self, delay: Duration, marker: &'static str, o: String) {
        let mut wire = Wire { sock: self.sock.try_clone().unwrap(), seen: self.seen.clone() };
        thread::spawn(move || {
            thread::sleep(delay);
            wire.mark(marker);
            wire.object(&o);
        });
    }
}

pub struct Fake {
    pub session: Session,
    seen: Receiver<(Instant, String)>,
    thread: JoinHandle<()>,
}

impl Fake {
    pub fn start(area: &'static str, mut extra: impl FnMut(&str, &mut Wire) + Send + 'static) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, seen) = mpsc::channel();
        let thread = thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut wire = Wire { sock, seen: tx };
            let mut pending = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = match wire.sock.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                pending.extend_from_slice(&chunk[..n]);
                while let Some(end) = pending.iter().position(|b| *b == 0) {
                    let msg = String::from_utf8(pending.drain(..=end).take(end).collect()).unwrap();
                    wire.mark(&msg);
                    if msg.contains("verChk") {
                        wire.send("<msg t='sys'><body action='apiOK' r='0'></body></msg>");
                    } else if msg.contains("action='login'") {
                        wire.send("%xt%loginResponse%-1%true%1001%hero%Welcome!%");
                    } else if msg.contains("%firstJoin%") {
                        wire.send("<msg t='sys'><body action='joinOK' r='5'><uLs r='5'><u i='1001'><n><![CDATA[hero]]></n></u></uLs></body></msg>");
                        wire.object(area);
                        wire.send("%xt%server%-1%You joined \"farm-1\"%");
                    } else if msg.contains("%retrieveInventory%") {
                        wire.object(SKILLS);
                        wire.object(r#"{"cmd":"loadInventoryBig","items":[],"factions":[]}"#);
                    } else if msg.contains("%logout%") {
                        wire.send("<msg t='sys'><body action='logout' r='0'></body></msg>");
                        return;
                    } else {
                        extra(&msg, &mut wire);
                    }
                }
            }
        });
        let server = Server { name: "Fake".into(), host: "127.0.0.1".into(), port, online: true, members_only: false, players: 0, capacity: 0 };
        let login = Login { api: "http://127.0.0.1:1", user: "hero", token: "t", version: "5.31" };
        let mut session = Session::connect(Control::new().unwrap(), &server, login, None, |_| {}).unwrap();
        session.pump_until(Some(Instant::now() + Duration::from_secs(5)), |s| (s.phase == Phase::Ready).then_some(())).unwrap();
        Fake { session, seen, thread }
    }

    pub fn finish(mut self) -> Vec<(Instant, String)> {
        self.session.logout();
        self.thread.join().unwrap();
        self.seen.try_iter().collect()
    }
}
