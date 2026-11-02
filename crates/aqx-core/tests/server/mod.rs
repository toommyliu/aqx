#![allow(dead_code)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aqx_core::{Control, Credentials, Session, Sink};

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
    {"ref":"a4","nam":"Finisher","cd":12000,"mp":30,"tgt":"h","tgtMax":1},
    {"ref":"i1","nam":"Item","cd":60000,"mp":0,"tgt":"f","tgtMax":1}]}}"#;

pub fn json(o: &str) -> String {
    format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#)
}

pub struct Wire {
    sock: TcpStream,
    seen: Sender<(Instant, String)>,
}

impl Wire {
    pub fn send(&mut self, msg: &str) {
        let _ = self.sock.write_all(msg.as_bytes());
        let _ = self.sock.write_all(b"\0");
    }

    pub fn object(&mut self, o: &str) {
        self.send(&json(o));
    }

    pub fn mark(&self, marker: &str) {
        let _ = self.seen.send((Instant::now(), marker.into()));
    }

    pub fn hang_up(&mut self) {
        self.mark("<hang up>");
        let _ = self.sock.shutdown(Shutdown::Both);
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

pub struct Servers {
    pub api: String,
    pub logins: Arc<AtomicUsize>,
    seen: Receiver<(Instant, String)>,
    done: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl Servers {
    pub fn start(area: &'static str, extra: impl FnMut(&str, &mut Wire) + Send + 'static) -> Servers {
        let game = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = TcpListener::bind("127.0.0.1:0").unwrap();
        game.set_nonblocking(true).unwrap();
        api.set_nonblocking(true).unwrap();
        let (game_port, api_port) = (game.local_addr().unwrap().port(), api.local_addr().unwrap().port());
        let (tx, seen) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        let logins = Arc::new(AtomicUsize::new(0));
        let threads = vec![
            thread::spawn({
                let (logins, done) = (logins.clone(), done.clone());
                move || serve_api(api, game_port, logins, done)
            }),
            thread::spawn({
                let done = done.clone();
                move || serve_game(game, area, extra, tx, done)
            }),
        ];
        Servers { api: format!("http://127.0.0.1:{api_port}"), logins, seen, done, threads }
    }

    pub fn credentials(&self) -> Credentials {
        Credentials { api: self.api.clone(), user: "hero".into(), password: "x".into() }
    }

    pub fn stop(self) -> Vec<(Instant, String)> {
        self.done.store(true, Ordering::SeqCst);
        for thread in self.threads {
            thread.join().unwrap();
        }
        self.seen.try_iter().collect()
    }
}

pub struct Fake {
    pub session: Session,
    pub logins: Arc<AtomicUsize>,
    servers: Servers,
}

fn accept(listener: &TcpListener, done: &AtomicBool) -> Option<TcpStream> {
    loop {
        if done.load(Ordering::SeqCst) {
            return None;
        }
        match listener.accept() {
            Ok((sock, _)) => {
                sock.set_nonblocking(false).unwrap();
                return Some(sock);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("accept failed: {e}"),
        }
    }
}

fn serve_api(listener: TcpListener, game: u16, logins: Arc<AtomicUsize>, done: Arc<AtomicBool>) {
    let servers = format!(r#"[{{"sName":"Fake","sIP":"127.0.0.1","iPort":{game},"iCount":1,"iMax":1000,"bOnline":1,"bUpg":0}}]"#);
    while let Some(mut sock) = accept(&listener, &done) {
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        let head_end = loop {
            match sock.read(&mut chunk) {
                Ok(0) | Err(_) => break None,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
            }
            if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                break Some(at + 4);
            }
        };
        let Some(head_end) = head_end else { continue };
        let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
        let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).map_or(0, |n| n.trim().parse().unwrap_or(0));
        while request.len() < head_end + length {
            match sock.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
            }
        }
        let body = if head.starts_with("post /game/api/login/now") {
            logins.fetch_add(1, Ordering::SeqCst);
            format!(r#"{{"login":{{"bSuccess":1,"sToken":"t","iUpg":0}},"servers":{servers}}}"#)
        } else {
            servers.clone()
        };
        let _ = write!(sock, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
    }
}

fn serve_game(listener: TcpListener, area: &'static str, mut extra: impl FnMut(&str, &mut Wire), seen: Sender<(Instant, String)>, done: Arc<AtomicBool>) {
    while let Some(sock) = accept(&listener, &done) {
        let mut wire = Wire { sock, seen: seen.clone() };
        let mut pending = Vec::new();
        let mut chunk = [0u8; 4096];
        'connection: loop {
            let n = match wire.sock.read(&mut chunk) {
                Ok(0) | Err(_) => break,
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
                    break 'connection;
                } else {
                    extra(&msg, &mut wire);
                }
            }
        }
    }
}

impl Fake {
    pub fn start(area: &'static str, extra: impl FnMut(&str, &mut Wire) + Send + 'static) -> Fake {
        Fake::start_with(area, extra, Rc::new(|_| {}))
    }

    pub fn start_with(area: &'static str, extra: impl FnMut(&str, &mut Wire) + Send + 'static, sink: Sink) -> Fake {
        let servers = Servers::start(area, extra);
        let mut session = Session::new(Control::new().unwrap(), servers.credentials(), "5.31", None, sink);
        session.login(Some("Fake")).unwrap();
        Fake { session, logins: servers.logins.clone(), servers }
    }

    pub fn finish(mut self) -> Vec<(Instant, String)> {
        self.session.logout();
        self.servers.stop()
    }
}
