#![allow(dead_code)]

use aqx_client::testing::{API_OK, Harness, Peer, TOKEN, json_message, login_reply};
use aqx_client::{Handle, Options, Reconnect, Server};
use aqx_protocol::{AreaId, EntityId, packet};
use aqx_script::skua::SkuaScript;
use aqx_script::{Account, Cancel, Outcome, Output, Start};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const FARM: &str = r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm","strMapFileName":"farm/farm.swf",
    "mondef":[{"MonID":10,"strMonName":"Mosquito"},{"MonID":12,"strMonName":"Treeant"}],
    "monmap":[{"MonMapID":1,"MonID":10,"strFrame":"r1"},{"MonMapID":2,"MonID":12,"strFrame":"r2"}],
    "monBranch":[{"MonMapID":1,"MonID":10,"intHP":540,"intHPMax":540,"intState":1},{"MonMapID":2,"MonID":12,"intHP":900,"intHPMax":900,"intState":1}],
    "uoBranch":[{"entID":1001,"strUsername":"player1","strFrame":"Enter","strPad":"Spawn","intHP":3000,"intHPMax":3000,"intMP":100,"intMPMax":100,"intState":1}]}"#;

pub fn data() -> PathBuf {
    PathBuf::from(
        std::env::var_os("AQX_SKUA_DATA").expect("scripts/skua-test.sh sets AQX_SKUA_DATA"),
    )
}

pub struct Dir(pub PathBuf);
impl Dir {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aqx-skua-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    pub fn script(&self, source: &str) -> PathBuf {
        let path = self.0.join("Main.cs");
        std::fs::write(&path, source).unwrap();
        path
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct NoAccount;
impl Account for NoAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        Err("no server list".into())
    }
    fn login(&self, _: &str) -> Result<Handle, String> {
        Err("no login".into())
    }
}
pub struct Wire<'a>(&'a mut Peer, bool);
impl Wire<'_> {
    pub fn send(&mut self, text: &str) {
        self.0.send(text);
    }
    pub fn hang_up(&mut self) {
        self.1 = true;
    }
    pub fn object(&mut self, text: &str) {
        self.0
            .send(&json_message(serde_json::from_str(text).unwrap()));
    }
}
pub struct Fake {
    harness: Harness,
    sent: Arc<Mutex<Vec<String>>>,
    api_stop: mpsc::Sender<()>,
    api: JoinHandle<()>,
}
impl Fake {
    pub fn start(response: impl FnMut(&str, &mut Wire) + Send + 'static) -> Self {
        Self::start_with(FARM, json!([]), json!([]), response)
    }
    pub fn start_with(
        map: &str,
        items: Value,
        bank: Value,
        response: impl FnMut(&str, &mut Wire) + Send + 'static,
    ) -> Self {
        Self::start_config(map, items, bank, json!({}), response)
    }
    pub fn start_config(
        map: &str,
        items: Value,
        bank: Value,
        data: Value,
        response: impl FnMut(&str, &mut Wire) + Send + 'static,
    ) -> Self {
        Self::start_options(Options::default(), map, items, bank, data, response)
    }
    pub fn reconnecting(response: impl FnMut(&str, &mut Wire) + Send + 'static) -> Self {
        let options = Options {
            reconnect: Some(Reconnect::new("pass1".into(), Duration::from_millis(100))),
            ..Options::default()
        };
        Self::start_options(options, FARM, json!([]), json!([]), json!({}), response)
    }
    fn start_options(
        options: Options,
        map: &str,
        items: Value,
        bank: Value,
        data: Value,
        mut response: impl FnMut(&str, &mut Wire) + Send + 'static,
    ) -> Self {
        let map = map.to_owned();
        let reconnects = options.reconnect.is_some();
        let (game_port, port) = mpsc::channel::<u16>();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (api_stop, stop) = mpsc::channel();
        let api = thread::spawn(move || {
            let mut game = None;
            while matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut length = 0;
                let mut map_file = false;
                let mut login = false;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    map_file |= line.starts_with("GET /gamefiles/maps/farm/farm.swf");
                    login |= line.starts_with("POST /login/now");
                    if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = if map_file {
                    map_swf()
                } else if login {
                    let game = *game.get_or_insert_with(|| port.recv().unwrap());
                    login_reply(TOKEN, game).into_bytes()
                } else {
                    bank.to_string().into_bytes()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let sent = Arc::new(Mutex::new(Vec::new()));
        let observed = sent.clone();
        let harness = Harness::start_with_api(options, base, move |listener| {
            game_port
                .send(listener.local_addr().unwrap().port())
                .unwrap();
            let mut next = Some(Peer::accept(&listener));
            while let Some(mut peer) = next.take() {
                peer.set_read_timeout(Duration::from_secs(30));
                peer.expect(&packet::version_check());
                peer.send(API_OK);
                peer.expect(&packet::login(
                    "player1",
                    aqx_client::DEFAULT_CLIENT_VERSION,
                    TOKEN,
                ));
                peer.send("%xt%loginResponse%-1%true%1001%player1%Welcome%0%sVersion=R0048%5.31%");
                peer.expect(&packet::first_join());
                peer.expect(&packet::clear_ignore_list());
                peer.send(&json_message(serde_json::from_str(&map).unwrap()));
                peer.send(
                    "<msg t='sys'><body action='joinOK' r='0'><uLs r='5'></uLs></body></msg>",
                );
                peer.send("%xt%server%-1%You joined \"farm-1\"!%");
                let first = peer.recv();
                observed.lock().unwrap().push(first.clone());
                assert!(first.contains("%moveToCell%"), "{first}");
                peer.expect(&packet::retrieve_user_data(AreaId(5), EntityId(1001)));
                let mut user =
                    json!({"intLevel":50,"intGold":1000,"intCoins":0,"iUpgDays":-1,"iBagSlots":30});
                user.as_object_mut()
                    .unwrap()
                    .extend(data.as_object().unwrap().clone());
                peer.send(&json_message(
                    json!({"cmd":"initUserData","uid":1001,"data":user}),
                ));
                peer.expect(&packet::retrieve_inventory(AreaId(5), EntityId(1001)));
                peer.send(&json_message(json!({"cmd":"sAct","actions":{"active":[
                    {"ref":"aa","nam":"Auto Attack","cd":2000,"mp":-10,"tgt":"h","tgtMax":1},
                    {"ref":"a1","nam":"Strike","cd":3000,"mp":10,"tgt":"h","tgtMax":1},
                    {"ref":"a2","nam":"Guard","cd":3000,"mp":10,"tgt":"s","tgtMax":1},
                    {"ref":"a3","nam":"Cleave","cd":6000,"mp":20,"tgt":"h","tgtMax":1},
                    {"ref":"a4","nam":"Finisher","cd":12000,"mp":30,"tgt":"h","tgtMax":1},
                    {"ref":"i1","nam":"Item","cd":60000,"mp":0,"tgt":"f","tgtMax":1}
                ]}})));
                peer.send(&json_message(
                    json!({"cmd":"loadInventoryBig","items":items,"hitems":[]}),
                ));
                let hung_up = loop {
                    let message = peer.recv();
                    observed.lock().unwrap().push(message.clone());
                    if message.contains("%cmd%1%logout%") {
                        break false;
                    }
                    let mut wire = Wire(&mut peer, false);
                    response(&message, &mut wire);
                    if wire.1 {
                        break true;
                    }
                };
                drop(peer);
                if hung_up && reconnects {
                    next = Peer::accept_within(&listener, Duration::from_secs(3));
                }
            }
        });
        harness.session.ready().wait().unwrap();
        Self {
            harness,
            sent,
            api_stop,
            api,
        }
    }
    pub fn run(self, source: &str) -> (Outcome, Vec<String>, Vec<String>) {
        self.run_with(source, &Cancel::default(), Default::default(), |_| {})
    }
    pub fn run_with(
        self,
        source: &str,
        cancel: &Cancel,
        args: serde_json::Map<String, Value>,
        observe: impl FnMut(&str) + 'static,
    ) -> (Outcome, Vec<String>, Vec<String>) {
        self.run_account(source, cancel, args, Arc::new(NoAccount), observe)
    }
    pub fn run_account(
        self,
        source: &str,
        cancel: &Cancel,
        args: serde_json::Map<String, Value>,
        account: Arc<dyn Account>,
        mut observe: impl FnMut(&str) + 'static,
    ) -> (Outcome, Vec<String>, Vec<String>) {
        let dir = Dir::new();
        let scripts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skua/vendor/Scripts");
        std::fs::copy(scripts.join("CoreBots.cs"), dir.0.join("CoreBots.cs")).unwrap();
        std::os::unix::fs::symlink(&scripts, dir.0.join("Scripts")).unwrap();
        for entry in std::fs::read_dir(&scripts).unwrap() {
            let entry = entry.unwrap();
            let target = dir.0.join(entry.file_name());
            if entry.path().extension().is_some_and(|e| e == "cs") && !target.exists() {
                std::os::unix::fs::symlink(entry.path(), target).unwrap();
            }
        }
        let path = dir.script(source);
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = logs.clone();
        let script = SkuaScript::load(
            &path,
            &data(),
            move |out| {
                if let Output::Log { text, .. } = out {
                    observe(&text);
                    sink.lock().unwrap().push(text);
                }
            },
            cancel,
        )
        .unwrap();
        let ended = script.run(Start {
            session: self.harness.session.clone(),
            server: "Test".into(),
            args,
            account,
        });
        if !matches!(
            ended.session.snapshot().phase,
            aqx_client::Phase::Closed { .. }
        ) {
            ended.session.ready().wait().unwrap();
        }
        ended.session.logout();
        self.harness.finish();
        let _ = self.api_stop.send(());
        self.api.join().unwrap();
        let logs = logs.lock().unwrap().clone();
        let sent = self.sent.lock().unwrap().clone();
        (ended.outcome, logs, sent)
    }
}

fn map_swf() -> Vec<u8> {
    use aqx_client::testing::MapCell;
    aqx_client::testing::map_swf_with(&[
        MapCell {
            name: "Enter",
            pads: &[],
            doors: &[(5, "r2", "Left")],
        },
        MapCell::named("r1"),
        MapCell {
            name: "r2",
            pads: &[("Top", 480, 300), ("Left", 190, 350)],
            doors: &[],
        },
        MapCell::named("Side1"),
        MapCell::named("Blank"),
    ])
}
