use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use aqx_protocol::{AreaId, Decoder, EntityId, Framer, Json, Message, Out, Xml, Xt};
use mio::net::TcpStream;
use mio::{Events, Interest, Token};
use serde::Deserialize;

use crate::api::Server;
use crate::control::Control;
use crate::record::Recorder;
use crate::world::{Area, Me, World};

pub const CLIENT_VERSION: &str = "5.31";

const SOCKET: Token = Token(0);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Handshaking,
    LoggingIn,
    Loading,
    Ready,
    Closed(Disconnect),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disconnect {
    ClientVersion(String),
    Rejected(String),
    LoggedOut,
    ConnectionLost(String),
    Protocol(String),
}

impl std::fmt::Display for Disconnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Disconnect::ClientVersion(v) => write!(f, "the server requires client version {v}"),
            Disconnect::Rejected(text) => write!(f, "login rejected: {text}"),
            Disconnect::LoggedOut => write!(f, "logged out"),
            Disconnect::ConnectionLost(e) => write!(f, "connection lost: {e}"),
            Disconnect::Protocol(e) => write!(f, "protocol error: {e}"),
        }
    }
}

#[derive(Debug)]
pub enum Halt {
    Timeout,
    Stopped,
    Closed(Disconnect),
}

#[derive(Debug)]
pub enum Event<'a> {
    LoggedIn { eid: EntityId },
    Joined { area: &'a str, players: usize },
    Ready { items: usize },
    ServerText(&'a str),
    Warning(&'a str),
    Unreadable(String),
    RecordingFailed(String),
}

pub struct Login<'a> {
    pub user: &'a str,
    pub token: &'a str,
    pub version: &'a str,
}

pub struct Session {
    control: Control,
    events: Events,
    stream: TcpStream,
    framer: Framer,
    decoder: Decoder,
    outbox: Vec<u8>,
    recorder: Option<Recorder>,
    sink: fn(&Event),
    login: (String, String, String),
    inventory_requested: bool,
    pub world: World,
    pub phase: Phase,
}

impl Session {
    pub fn connect(
        control: Control,
        server: &Server,
        login: Login,
        recorder: Option<Recorder>,
        sink: fn(&Event),
    ) -> io::Result<Session> {
        let addr = (server.host.as_str(), server.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} has no address", server.host)))?;
        let std_stream = std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        std_stream.set_nodelay(true)?;
        std_stream.set_nonblocking(true)?;
        let mut stream = TcpStream::from_std(std_stream);
        control.poll.registry().register(&mut stream, SOCKET, Interest::READABLE | Interest::WRITABLE)?;
        let mut session = Session {
            control,
            events: Events::with_capacity(16),
            stream,
            framer: Framer::default(),
            decoder: Decoder::default(),
            outbox: Vec::new(),
            recorder,
            sink,
            login: (login.user.into(), login.token.into(), login.version.into()),
            inventory_requested: false,
            world: World::default(),
            phase: Phase::Handshaking,
        };
        session.send(Out::VerChk);
        Ok(session)
    }

    pub fn into_parts(mut self) -> (Control, Option<Recorder>) {
        let _ = self.control.poll.registry().deregister(&mut self.stream);
        (self.control, self.recorder)
    }

    pub fn pump_until<T>(&mut self, deadline: Option<Instant>, mut done: impl FnMut(&Session) -> Option<T>) -> Result<T, Halt> {
        loop {
            if let Some(v) = done(self) {
                return Ok(v);
            }
            if let Phase::Closed(why) = &self.phase {
                return Err(Halt::Closed(why.clone()));
            }
            if self.control.stop_requested() {
                return Err(Halt::Stopped);
            }
            let timeout = match deadline {
                Some(d) => match d.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => Some(left),
                    _ => return Err(Halt::Timeout),
                },
                None => None,
            };
            match self.control.poll.poll(&mut self.events, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => self.close(Disconnect::ConnectionLost(e.to_string())),
            }
            self.read_socket();
            self.flush();
        }
    }

    pub fn logout(&mut self) {
        self.control.clear_stop();
        if matches!(self.phase, Phase::Closed(_)) {
            return;
        }
        self.send(Out::Logout);
        let _ = self.pump_until(Some(Instant::now() + Duration::from_secs(3)), |_| None::<()>);
    }

    fn send(&mut self, out: Out) {
        let start = self.outbox.len();
        out.encode(&mut self.outbox);
        let sent = String::from_utf8_lossy(&self.outbox[start..self.outbox.len() - 1]).into_owned();
        self.record("out", &sent);
        self.flush();
    }

    fn flush(&mut self) {
        while !self.outbox.is_empty() && !matches!(self.phase, Phase::Closed(_)) {
            match self.stream.write(&self.outbox) {
                Ok(0) => self.close(Disconnect::ConnectionLost("socket closed while writing".into())),
                Ok(n) => {
                    self.outbox.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => self.close(Disconnect::ConnectionLost(e.to_string())),
            }
        }
    }

    fn read_socket(&mut self) {
        let mut buf = [0u8; 16 * 1024];
        while !matches!(self.phase, Phase::Closed(_)) {
            match self.stream.read(&mut buf) {
                Ok(0) => self.close(Disconnect::ConnectionLost("server closed the connection".into())),
                Ok(n) => self.receive_bytes(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => self.close(Disconnect::ConnectionLost(e.to_string())),
            }
        }
    }

    fn receive_bytes(&mut self, data: &[u8]) {
        let mut framer = std::mem::take(&mut self.framer);
        let mut decoder = std::mem::take(&mut self.decoder);
        let fed = framer.feed(data, |frame| match decoder.decode(frame) {
            Ok(text) => self.receive(text),
            Err(e) => (self.sink)(&Event::Unreadable(format!("{e:?}"))),
        });
        self.framer = framer;
        self.decoder = decoder;
        if let Err(e) = fed {
            self.close(Disconnect::Protocol(format!("{e:?}")));
        }
    }

    fn receive(&mut self, text: &str) {
        if matches!(self.phase, Phase::Closed(_)) {
            return;
        }
        self.record("in", text);
        if self.phase != Phase::Ready
            && let Some(version) = required_version(text)
        {
            self.close(Disconnect::ClientVersion(version));
            return;
        }
        match Message::parse(text) {
            Ok(Message::Xml(xml)) => self.on_xml(xml),
            Ok(Message::Json(json)) => self.on_json(json),
            Ok(Message::Xt(xt)) => self.on_xt(xt),
            Err(e) => (self.sink)(&Event::Unreadable(format!("{e:?}: {text:.80}"))),
        }
    }

    fn on_xml(&mut self, xml: Xml) {
        match xml.action {
            Some("apiOK") if self.phase == Phase::Handshaking => {
                let (user, token, version) = self.login.clone();
                self.send(Out::Login { user: &user, version: &version, token: &token });
                self.phase = Phase::LoggingIn;
            }
            Some("joinOK") => {
                let me = self.world.me.as_ref().map(|m| m.eid);
                self.world.others = xml.user_ids().filter(|id| Some(*id) != me).collect();
            }
            Some("uER") => self.world.others.extend(xml.user_ids()),
            Some("userGone") => {
                let gone: Vec<_> = xml.user_ids().collect();
                self.world.others.retain(|id| !gone.contains(id));
            }
            Some("logout") => self.close(Disconnect::LoggedOut),
            _ => {}
        }
    }

    fn on_xt(&mut self, xt: Xt) {
        match xt.cmd {
            "loginResponse" if self.phase == Phase::LoggingIn => {
                let eid = xt.arg(1).and_then(|s| s.parse().ok()).map(EntityId);
                match (xt.arg(0), eid) {
                    (Some("true"), Some(eid)) => {
                        let user = xt.arg(2).unwrap_or(&self.login.0).to_string();
                        (self.sink)(&Event::LoggedIn { eid });
                        self.world.me = Some(Me { eid, user, char_id: None });
                        self.send(Out::FirstJoin);
                        self.send(Out::ClearIgnoreList);
                        self.phase = Phase::Loading;
                    }
                    _ => {
                        let text = xt.args().filter(|a| !a.is_empty()).last().unwrap_or("no reason given");
                        self.close(Disconnect::Rejected(text.to_string()));
                    }
                }
            }
            "server" => {
                let text = xt.arg(0).unwrap_or("");
                if text.contains("You joined") {
                    self.on_joined();
                } else {
                    (self.sink)(&Event::ServerText(text));
                }
            }
            "warning" => (self.sink)(&Event::Warning(xt.arg(0).unwrap_or(""))),
            _ => {}
        }
    }

    fn on_joined(&mut self) {
        let (Some(me), Some(area)) = (&self.world.me, &self.world.area) else { return };
        let (eid, room) = (me.eid, area.id);
        (self.sink)(&Event::Joined { area: &area.name, players: self.world.others.len() + 1 });
        self.send(Out::MoveToCell { room, cell: "Enter", pad: "Spawn" });
        self.send(Out::RetrieveUserData { room, eid });
        if !self.world.others.is_empty() {
            let others = self.world.others.clone();
            self.send(Out::RetrieveUserDatas { room, others: &others });
        }
        if !self.inventory_requested {
            self.inventory_requested = true;
            self.send(Out::RetrieveInventory { room, eid });
        }
    }

    fn on_json(&mut self, json: Json) {
        match json.cmd {
            "moveToArea" => {
                #[derive(Deserialize)]
                struct MoveToArea<'a> {
                    #[serde(rename = "areaId")]
                    id: AreaId,
                    #[serde(rename = "areaName")]
                    name: &'a str,
                    #[serde(rename = "strMapName")]
                    map: &'a str,
                }
                match json.decode::<MoveToArea>() {
                    Ok(m) => self.world.area = Some(Area { id: m.id, name: m.name.into(), map: m.map.into() }),
                    Err(e) => (self.sink)(&Event::Unreadable(format!("moveToArea: {e}"))),
                }
            }
            "initUserData" => {
                #[derive(Deserialize)]
                struct InitUserData {
                    uid: EntityId,
                    data: Data,
                }
                #[derive(Deserialize)]
                struct Data {
                    #[serde(rename = "CharID")]
                    char_id: u32,
                }
                if let (Ok(init), Some(me)) = (json.decode::<InitUserData>(), &mut self.world.me)
                    && init.uid == me.eid
                {
                    me.char_id = Some(init.data.char_id);
                }
            }
            "loadInventoryBig" => {
                #[derive(Deserialize)]
                struct Inventory<'a> {
                    #[serde(borrow)]
                    items: Vec<&'a serde_json::value::RawValue>,
                }
                self.world.items = json.decode::<Inventory>().map(|i| i.items.len()).unwrap_or(0);
                if self.phase == Phase::Loading {
                    self.phase = Phase::Ready;
                    (self.sink)(&Event::Ready { items: self.world.items });
                }
            }
            _ => {}
        }
    }

    fn record(&mut self, dir: &str, text: &str) {
        if let Some(recorder) = &mut self.recorder
            && let Err(e) = recorder.write(dir, text)
        {
            self.recorder = None;
            (self.sink)(&Event::RecordingFailed(e.to_string()));
        }
    }

    fn close(&mut self, why: Disconnect) {
        if !matches!(self.phase, Phase::Closed(_)) {
            self.phase = Phase::Closed(why);
        }
    }
}

fn required_version(text: &str) -> Option<String> {
    const MARK: &str = "requires AQW game version ";
    let rest = &text[text.find(MARK)? + MARK.len()..];
    let version = rest[..rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len())].trim_end_matches('.');
    (!version.is_empty()).then(|| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_required_version_from_the_rejection_text() {
        let text = "%xt%loginResponse%-1%false%-1%hero%Your game client is an older version. Server requires AQW game version 5.31 or higher. Please close the AQW game window and try again.%";
        assert_eq!(required_version(text).as_deref(), Some("5.31"));
        assert_eq!(required_version("%xt%server%-1%You joined \"battleon-3\"%"), None);
    }
}
