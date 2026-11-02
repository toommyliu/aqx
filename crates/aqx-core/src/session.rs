use std::io::{self, Read, Write};
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use aqx_protocol::{Decoder, Framer, Message, Out};
use mio::net::TcpStream;
use mio::{Events, Interest, Token};

use crate::api::Server;
use crate::control::Control;
use crate::event::Event;
use crate::record::Recorder;
use crate::world::{self, World};

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
    LoggedInElsewhere,
    ConnectionLost(String),
    Protocol(String),
}

impl std::fmt::Display for Disconnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Disconnect::ClientVersion(v) => write!(f, "the server requires client version {v}"),
            Disconnect::Rejected(text) => write!(f, "login rejected: {text}"),
            Disconnect::LoggedOut => write!(f, "logged out"),
            Disconnect::LoggedInElsewhere => write!(f, "this account logged in somewhere else"),
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
            Err(e) => (self.sink)(&Event::Unreadable { what: format!("{e:?}") }),
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
        let msg = match Message::parse(text) {
            Ok(msg) => msg,
            Err(e) => return (self.sink)(&Event::Unreadable { what: format!("{e:?}: {text:.80}") }),
        };
        if let Message::Xml(xml) = &msg {
            match xml.action {
                Some("apiOK") if self.phase == Phase::Handshaking => {
                    let (user, token, version) = self.login.clone();
                    self.send(Out::Login { user: &user, version: &version, token: &token });
                    self.phase = Phase::LoggingIn;
                }
                Some("logout") => return self.close(Disconnect::LoggedOut),
                _ => {}
            }
        }
        for event in world::reduce(&mut self.world, &msg, Instant::now()) {
            (self.sink)(&event);
            self.react(&event);
        }
    }

    fn react(&mut self, event: &Event) {
        match event {
            Event::LoggedIn { .. } if self.phase == Phase::LoggingIn => {
                if let Some(me) = self.world.me.as_mut().filter(|me| me.user.is_empty()) {
                    me.user.clone_from(&self.login.0);
                }
                self.send(Out::FirstJoin);
                self.send(Out::ClearIgnoreList);
                self.phase = Phase::Loading;
            }
            Event::LoginRejected { reason } => self.close(Disconnect::Rejected(reason.clone())),
            Event::LoggedInElsewhere => self.close(Disconnect::LoggedInElsewhere),
            Event::Joined { .. } => self.on_joined(),
            Event::InventoryLoaded { items } if self.phase == Phase::Loading => {
                self.phase = Phase::Ready;
                (self.sink)(&Event::Ready { items: *items });
            }
            _ => {}
        }
    }

    fn on_joined(&mut self) {
        let (Some(me), Some(area)) = (&mut self.world.me, &self.world.area) else { return };
        me.cell = "Enter".into();
        me.pad = "Spawn".into();
        let (eid, room) = (me.eid, area.id);
        let others: Vec<_> = area.players.iter().map(|p| p.eid).collect();
        self.send(Out::MoveToCell { room, cell: "Enter", pad: "Spawn" });
        self.send(Out::RetrieveUserData { room, eid });
        if !others.is_empty() {
            self.send(Out::RetrieveUserDatas { room, others: &others });
        }
        if !self.inventory_requested {
            self.inventory_requested = true;
            self.send(Out::RetrieveInventory { room, eid });
        }
    }

    fn record(&mut self, dir: &str, text: &str) {
        if let Some(recorder) = &mut self.recorder
            && let Err(e) = recorder.write(dir, text)
        {
            self.recorder = None;
            (self.sink)(&Event::RecordingFailed { error: e.to_string() });
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
