use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use aqx_client::{ActionError, ActionId, Direction, Event, Handle, Phase, Server, Update, World};
use rquickjs::function::This;
use rquickjs::{
    Array, CatchResultExt, CaughtError, Coerced, Constructor, Ctx, Error, FromJs, Function, Module,
    Object, Value,
};

use crate::act::{self, Act, Answer, Reply, Started};
use crate::native::{Host, Internals};
use crate::{
    Account, Cancel, External, LoadError, Outcome, Relogged, Start, Wake, events, forward, restore,
    source, views,
};

const MAIN: TaskId = TaskId(0);
const UNWIND_LIMIT: usize = 64;
const HANDLER_LIMIT: usize = 100;
const ACTION_LIMIT: usize = 100;
const PREDICATE_POLL: Duration = Duration::from_millis(100);
const INTERNAL_FRAME: &str = "(aqx";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct TaskId(u64);

enum Origin {
    Main,
    Child(TaskId, String),
    Handler(u64),
}

struct Task<'js> {
    generator: Object<'js>,
    origin: Origin,
    wait: Wait<'js>,
}

#[derive(Clone, PartialEq, Eq)]
struct Topic {
    json: bool,
    cmd: String,
}

enum Wait<'js> {
    Running,
    Timer(u64),
    First(Vec<TaskId>),
    Action(ActionId, Answer),
    Event(String, Option<Function<'js>>),
    Packet(Topic, Option<Function<'js>>),
    Until(Function<'js>),
    External(u64),
}

enum Resume<'js> {
    Next(Value<'js>),
    Throw(Value<'js>),
}

enum Op<'js> {
    Ms(Duration),
    First(Vec<(String, Object<'js>)>),
    Act(Act),
    Until(Function<'js>),
    Once(String, Option<Function<'js>>),
    Packet(Topic, Option<Function<'js>>),
    Servers,
    Relogin(Option<String>),
    Logout,
}

enum End<'js> {
    Returned,
    Threw(Value<'js>),
}

pub struct Engine<'js> {
    ctx: Ctx<'js>,
    session: Handle,
    internals: Internals<'js>,
    next: Function<'js>,
    throw: Function<'js>,
    finish: Function<'js>,
    type_error: Constructor<'js>,
    tasks: HashMap<TaskId, Task<'js>>,
    ids: u64,
    ready: VecDeque<(TaskId, Resume<'js>)>,
    timers: BinaryHeap<Reverse<(Instant, u64, TaskId)>>,
    timer_seq: u64,
    actions: HashMap<ActionId, TaskId>,
    closed: Option<String>,
    end: Option<End<'js>>,
    account: Arc<dyn Account>,
    server: String,
    wake: mpsc::Sender<Wake>,
    generation: u64,
    externals: HashMap<u64, TaskId>,
    next_external: u64,
    link: Link,
    logged_out: bool,
}

enum Link {
    Live,
    Relogging,
    Lost,
}

pub struct Loaded<'js> {
    main: Function<'js>,
    next: Function<'js>,
    throw: Function<'js>,
    finish: Function<'js>,
    type_error: Constructor<'js>,
    internals: Internals<'js>,
}

pub fn load<'js>(ctx: &Ctx<'js>, path: &Path) -> Result<Loaded<'js>, LoadError> {
    let failed = |e| LoadError(caught(ctx, e));
    let generator_function: Object = ctx
        .eval("Object.getPrototypeOf(function* () {})")
        .map_err(failed)?;
    let generator_prototype: Object = generator_function.get("prototype").map_err(failed)?;
    let generator_constructor: Constructor =
        generator_function.get("constructor").map_err(failed)?;
    let method = |name| generator_prototype.get::<_, Function>(name).map_err(failed);
    let (next, throw, finish) = (method("next")?, method("throw")?, method("return")?);
    let type_error: Constructor = ctx.globals().get("TypeError").map_err(failed)?;

    Module::import(ctx, "aqx")
        .and_then(|loaded| loaded.finish::<Value>())
        .map_err(failed)?;
    let internals = ctx
        .userdata::<Host>()
        .and_then(|host| host.take_internals())
        .expect("loading the aqx module hands its internals to the host");
    let name = path.to_string_lossy().into_owned();
    let code = source::read(path).map_err(|why| LoadError(format!("{name}: {why}")))?;
    let main = Module::declare(ctx.clone(), name, code)
        .and_then(|module| module.eval())
        .and_then(|(module, promise)| promise.finish::<()>().map(|()| module))
        .and_then(|module| module.get::<_, Value>("default"))
        .map_err(failed)?;
    let main = main
        .into_function()
        .filter(|f| {
            f.as_object()
                .is_some_and(|f| f.is_instance_of(&generator_constructor))
        })
        .ok_or_else(|| {
            LoadError("the script's default export must be a generator function".into())
        })?;
    Ok(Loaded {
        main,
        next,
        throw,
        finish,
        type_error,
        internals,
    })
}

impl<'js> Engine<'js> {
    pub fn start(
        ctx: &Ctx<'js>,
        loaded: Loaded<'js>,
        start: &Start,
        wake: mpsc::Sender<Wake>,
    ) -> rquickjs::Result<Engine<'js>> {
        let main = loaded.main.call::<_, Object>(())?;
        let mut tasks = HashMap::new();
        tasks.insert(
            MAIN,
            Task {
                generator: main,
                origin: Origin::Main,
                wait: Wait::Running,
            },
        );
        Ok(Engine {
            ctx: ctx.clone(),
            session: start.session.clone(),
            account: Arc::clone(&start.account),
            server: start.server.clone(),
            wake,
            generation: 0,
            externals: HashMap::new(),
            next_external: 0,
            link: Link::Live,
            logged_out: false,
            internals: loaded.internals,
            next: loaded.next,
            throw: loaded.throw,
            finish: loaded.finish,
            type_error: loaded.type_error,
            tasks,
            ids: 1,
            ready: VecDeque::from([(MAIN, Resume::Next(Value::new_undefined(ctx.clone())))]),
            timers: BinaryHeap::new(),
            timer_seq: 0,
            actions: HashMap::new(),
            closed: None,
            end: None,
        })
    }

    pub fn into_session(self) -> (Handle, String) {
        (self.session, self.server)
    }

    pub fn run(&mut self, inbox: &mpsc::Receiver<Wake>, cancel: &Cancel) -> Outcome {
        let driven = self.drive(inbox, cancel);
        cancel.resume_cleanup();
        let roots: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|(_, task)| !matches!(task.origin, Origin::Child(..)))
            .map(|(id, _)| *id)
            .collect();
        let halted = roots.into_iter().try_for_each(|id| self.halt(id));
        let (stopped, restart) = match self.ctx.userdata::<Host>() {
            Some(host) => (host.stop_requested(), host.restart_requested()),
            None => (None, false),
        };
        if let Some(reason) = stopped {
            return Outcome::Stopped(reason);
        }
        if cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        if self.logged_out {
            return Outcome::LoggedOut;
        }
        if restart {
            return Outcome::Restart;
        }
        if let Some(reason) = self.closed.take() {
            return Outcome::Disconnected(reason);
        }
        if let Err(e) = driven.and(halted) {
            return Outcome::Failed(caught(&self.ctx, e));
        }
        match self.end.take() {
            Some(End::Threw(value)) => Outcome::Failed(describe(&self.ctx, value)),
            Some(End::Returned) | None => Outcome::Finished,
        }
    }

    fn drive(&mut self, inbox: &mpsc::Receiver<Wake>, cancel: &Cancel) -> rquickjs::Result<()> {
        let mut seen = self.session.snapshot();
        let mut held = VecDeque::new();
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            if cancel.is_paused() {
                match inbox.recv() {
                    Ok(Wake::Cancel) | Err(_) => return Ok(()),
                    Ok(Wake::Resume) => {}
                    Ok(Wake::Update(_, Update::State | Update::Packet(Direction::Out, _))) => {}
                    Ok(Wake::Update(_, Update::Packet(Direction::In, _)))
                        if !self.wants_packets() => {}
                    Ok(Wake::Update(
                        generation,
                        Update::Event(Event::Phase(
                            Phase::Closed { reason } | Phase::Reconnecting { reason, .. },
                        )),
                    )) if generation == self.generation => {
                        self.closed = Some(reason.to_string());
                        return Ok(());
                    }
                    Ok(wake) => held.push_back(wake),
                }
                continue;
            }
            self.run_ready()?;
            if self.over() || cancel.is_cancelled() {
                return Ok(());
            }
            let poll = self
                .predicates_waiting()
                .then(|| Instant::now() + PREDICATE_POLL);
            let wake = match (
                held.pop_front(),
                self.next_deadline().into_iter().chain(poll).min(),
            ) {
                (Some(wake), _) => Ok(wake),
                (None, Some(at)) => {
                    inbox.recv_timeout(at.saturating_duration_since(Instant::now()))
                }
                (None, None) => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            let mut changed = true;
            match wake {
                Ok(Wake::Cancel) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(Wake::Resume) => continue,
                Ok(Wake::Update(generation, _)) if generation != self.generation => continue,
                Ok(Wake::Update(
                    _,
                    Update::Event(Event::Phase(
                        Phase::Closed { reason } | Phase::Reconnecting { reason, .. },
                    )),
                )) => {
                    self.closed = Some(reason.to_string());
                    return Ok(());
                }
                Ok(Wake::Update(_, Update::Event(event))) => self.on_event(&event)?,
                Ok(Wake::Update(_, Update::Packet(Direction::In, text))) => {
                    self.on_packet(&text)?
                }
                Ok(Wake::Update(_, Update::State)) => continue,
                Ok(Wake::Update(_, Update::Packet(Direction::Out, _))) => {}
                Ok(Wake::Update(
                    _,
                    Update::Done {
                        result: Err(ActionError::Closed(reason)),
                        ..
                    },
                )) => {
                    self.closed = Some(reason.to_string());
                    return Ok(());
                }
                Ok(Wake::Update(_, Update::Done { id, result })) => self.answer(id, result)?,
                Ok(Wake::External(id, external)) => {
                    self.on_external(id, external)?;
                    if self.closed.is_some() {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => changed = false,
            }
            self.fire_timers(Instant::now());
            let latest = self.session.snapshot();
            if changed || !Arc::ptr_eq(&latest, &seen) {
                seen = latest;
                self.check_predicates()?;
            }
        }
    }

    fn run_ready(&mut self) -> rquickjs::Result<()> {
        while let Some((id, resume)) = self.ready.pop_front() {
            self.step(id, resume)?;
            self.settle_js()?;
            if self.over() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn settle_js(&mut self) -> rquickjs::Result<()> {
        while self.ctx.execute_pending_job() {}
        loop {
            let (spawned, halted) = match self.ctx.userdata::<Host>() {
                Some(host) => (host.take_spawned(), host.take_halted()),
                None => return Ok(()),
            };
            if spawned.is_empty() && halted.is_empty() {
                break;
            }
            for tag in halted {
                let tagged: Vec<TaskId> = self
                    .tasks
                    .iter()
                    .filter(|(_, task)| matches!(task.origin, Origin::Handler(t) if t == tag))
                    .map(|(id, _)| *id)
                    .collect();
                for id in tagged {
                    self.halt(id)?;
                }
            }
            for (tag, generator) in spawned {
                self.spawn(generator, Origin::Handler(tag));
            }
        }
        let handlers = self
            .tasks
            .values()
            .filter(|task| matches!(task.origin, Origin::Handler(_)))
            .count();
        if handlers > HANDLER_LIMIT {
            self.fail(&format!(
                "more than {HANDLER_LIMIT} handler calls are pending"
            ))?;
        }
        if self.actions.len() + self.externals.len() > ACTION_LIMIT {
            self.fail(&format!("more than {ACTION_LIMIT} actions are pending"))?;
        }
        Ok(())
    }

    fn fail(&mut self, message: &str) -> rquickjs::Result<()> {
        let error = self.aqx_error(message)?;
        self.end.get_or_insert(End::Threw(error));
        Ok(())
    }

    fn see(&self) -> Arc<World> {
        let world = self.session.snapshot();
        if let Some(host) = self.ctx.userdata::<Host>() {
            host.see(Arc::clone(&world));
        }
        world
    }

    fn on_event(&mut self, event: &Event) -> rquickjs::Result<()> {
        let world = self.see();
        let Some((name, payload)) = events::payload(&self.ctx, &world, event)? else {
            return Ok(());
        };
        let started = self
            .internals
            .dispatch_event
            .call::<_, Value>((name, payload.clone()));
        self.start_handlers(started)?;
        let waiting: Vec<(TaskId, Option<Function<'js>>)> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| match &task.wait {
                Wait::Event(wanted, filter) if wanted == name => Some((*id, filter.clone())),
                _ => None,
            })
            .collect();
        for (id, filter) in waiting {
            self.offer(id, filter, payload.clone().into_value())?;
        }
        self.settle_js()
    }

    fn wants_packets(&self) -> bool {
        let waited = self
            .tasks
            .values()
            .any(|task| matches!(task.wait, Wait::Packet(..)));
        waited
            || self
                .ctx
                .userdata::<Host>()
                .is_some_and(|host| host.watches_packets())
    }

    fn on_packet(&mut self, text: &str) -> rquickjs::Result<()> {
        if !self.wants_packets() {
            return Ok(());
        }
        let Some((topic, packet)) = self.parse_packet(text)? else {
            return Ok(());
        };
        self.see();
        let kind = if topic.json { "json" } else { "xt" };
        let started = self.internals.dispatch_packet.call::<_, Value>((
            kind,
            topic.cmd.as_str(),
            packet.clone(),
        ));
        self.start_handlers(started)?;
        let waiting: Vec<(TaskId, Option<Function<'js>>)> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| match &task.wait {
                Wait::Packet(wanted, filter) if *wanted == topic => Some((*id, filter.clone())),
                _ => None,
            })
            .collect();
        for (id, filter) in waiting {
            self.offer(id, filter, packet.clone().into_value())?;
        }
        self.settle_js()
    }

    fn parse_packet(&self, text: &str) -> rquickjs::Result<Option<(Topic, Object<'js>)>> {
        let packet = Object::new(self.ctx.clone())?;
        if text.starts_with('{') {
            let root = match self.ctx.json_parse(text) {
                Ok(root) => root,
                Err(Error::Exception) => {
                    self.ctx.catch();
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };
            let data = root
                .as_object()
                .and_then(|root| root.get::<_, Object>("b").ok())
                .and_then(|body| body.get::<_, Object>("o").ok());
            let Some(data) = data else {
                return Ok(None);
            };
            let Ok(cmd) = data.get::<_, String>("cmd") else {
                return Ok(None);
            };
            packet.set("cmd", cmd.as_str())?;
            packet.set("data", data)?;
            return Ok(Some((Topic { json: true, cmd }, packet)));
        }
        let fields: Vec<&str> = text.split('%').collect();
        if !text.starts_with('%') || fields.len() < 4 || fields[1] != "xt" || fields[2].is_empty() {
            return Ok(None);
        }
        let mut args = &fields[4.min(fields.len())..];
        if let Some((&"", rest)) = args.split_last() {
            args = rest;
        }
        let cmd = fields[2].to_owned();
        let list = Array::new(self.ctx.clone())?;
        for (i, arg) in args.iter().enumerate() {
            list.set(i, *arg)?;
        }
        packet.set("cmd", cmd.as_str())?;
        packet.set("args", list)?;
        Ok(Some((Topic { json: false, cmd }, packet)))
    }

    fn start_handlers(&mut self, started: rquickjs::Result<Value<'js>>) -> rquickjs::Result<()> {
        let started = match started {
            Ok(started) => started,
            Err(Error::Exception) => {
                let thrown = self.ctx.catch();
                self.end.get_or_insert(End::Threw(thrown));
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let Some(started) = started.into_array() else {
            return Ok(());
        };
        for entry in started.iter::<Array>() {
            let entry = entry?;
            let tag: f64 = entry.get(0)?;
            self.spawn(entry.get(1)?, Origin::Handler(tag as u64));
        }
        Ok(())
    }

    fn offer(
        &mut self,
        id: TaskId,
        filter: Option<Function<'js>>,
        value: Value<'js>,
    ) -> rquickjs::Result<()> {
        if !self.tasks.contains_key(&id) {
            return Ok(());
        }
        let accepted = match filter {
            None => Ok(true),
            Some(filter) => filter
                .call::<_, Coerced<bool>>((value.clone(),))
                .map(|accepted| accepted.0),
        };
        let resume = match accepted {
            Ok(true) => Resume::Next(value),
            Ok(false) => return Ok(()),
            Err(Error::Exception) => Resume::Throw(self.ctx.catch()),
            Err(e) => return Err(e),
        };
        self.wait(id, Wait::Running);
        self.ready.push_back((id, resume));
        Ok(())
    }

    fn predicates_waiting(&self) -> bool {
        self.tasks
            .values()
            .any(|task| matches!(task.wait, Wait::Until(_)))
    }

    fn check_predicates(&mut self) -> rquickjs::Result<()> {
        let waiting: Vec<(TaskId, Function<'js>)> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| match &task.wait {
                Wait::Until(predicate) => Some((*id, predicate.clone())),
                _ => None,
            })
            .collect();
        if waiting.is_empty() {
            return Ok(());
        }
        self.see();
        for (id, predicate) in waiting {
            let undefined = Value::new_undefined(self.ctx.clone());
            self.offer(id, Some(predicate), undefined)?;
        }
        self.settle_js()
    }

    fn answer(&mut self, id: ActionId, result: Result<(), ActionError>) -> rquickjs::Result<()> {
        let Some(task) = self.actions.remove(&id) else {
            return Ok(());
        };
        let Some(Wait::Action(_, answer)) = self.tasks.get(&task).map(|t| &t.wait) else {
            return Ok(());
        };
        let resume = match answer.reply(result) {
            Ok(Reply::Nothing) => Resume::Next(Value::new_undefined(self.ctx.clone())),
            Ok(Reply::Text(text)) => Resume::Next(self.string(text)?),
            Ok(Reply::View(name)) => {
                let world = self.session.snapshot();
                Resume::Next(views::view(&self.ctx, &world, name)?)
            }
            Err(message) => Resume::Throw(self.aqx_error(&message)?),
        };
        self.wait(task, Wait::Running);
        self.ready.push_back((task, resume));
        Ok(())
    }

    fn string(&self, text: &str) -> rquickjs::Result<Value<'js>> {
        rquickjs::String::from_str(self.ctx.clone(), text).map(|s| s.into_value())
    }

    fn aqx_error(&self, message: &str) -> rquickjs::Result<Value<'js>> {
        self.internals.aqx_error.construct((message,))
    }

    fn over(&self) -> bool {
        self.end.is_some()
            || self.logged_out
            || self
                .ctx
                .userdata::<Host>()
                .is_some_and(|host| host.stop_requested().is_some() || host.restart_requested())
    }

    fn external(&mut self, task: TaskId) -> u64 {
        let id = self.next_external;
        self.next_external += 1;
        self.externals.insert(id, task);
        self.wait(task, Wait::External(id));
        id
    }

    fn start_relogin(&mut self, task: TaskId, server: Option<String>) -> rquickjs::Result<()> {
        if matches!(self.link, Link::Relogging) {
            let error = self.aqx_error("the session is already relogging")?;
            self.ready.push_back((task, Resume::Throw(error)));
            return Ok(());
        }
        self.link = Link::Relogging;
        self.generation += 1;
        let in_flight: Vec<(ActionId, TaskId)> = self.actions.drain().collect();
        for (action, waiting) in in_flight {
            self.session.cancel(action);
            let error = self.aqx_error("the session relogged before this action finished")?;
            self.wait(waiting, Wait::Running);
            self.ready.push_back((waiting, Resume::Throw(error)));
        }
        self.session.logout();
        let server = server.unwrap_or_else(|| self.server.clone());
        let id = self.external(task);
        let (account, wake) = (Arc::clone(&self.account), self.wake.clone());
        thread::spawn(move || {
            let relogged = account
                .login(&server)
                .map(|session| Relogged(Some((session, server))));
            let _ = wake.send(Wake::External(id, External::Relogged(relogged)));
        });
        Ok(())
    }

    fn on_external(&mut self, id: u64, external: External) -> rquickjs::Result<()> {
        let task = self
            .externals
            .remove(&id)
            .filter(|task| matches!(self.tasks.get(task).map(|t| &t.wait), Some(Wait::External(e)) if *e == id));
        let resume = match external {
            External::Servers(Ok(servers)) => Resume::Next(self.servers(&servers)?),
            External::Servers(Err(message)) => Resume::Throw(self.aqx_error(&message)?),
            External::Relogged(Ok(relogged)) => {
                let (session, server) = relogged.take();
                if let Err(e) = forward(&session, self.generation, self.wake.clone()) {
                    session.logout();
                    self.closed = Some(format!("couldn't follow the new session: {e}"));
                    return Ok(());
                }
                self.link = Link::Live;
                if let Some(host) = self.ctx.userdata::<Host>() {
                    restore(&session, host.settings());
                    host.attach(session.clone());
                    host.set_server(&server);
                }
                self.session = session;
                self.server = server;
                Resume::Next(Value::new_undefined(self.ctx.clone()))
            }
            External::Relogged(Err(message)) => {
                self.link = Link::Lost;
                let message = format!("couldn't log back in: {message}");
                if task.is_none() {
                    self.closed = Some(message);
                    return Ok(());
                }
                Resume::Throw(self.aqx_error(&message)?)
            }
        };
        if let Some(task) = task {
            self.wait(task, Wait::Running);
            self.ready.push_back((task, resume));
        }
        Ok(())
    }

    fn servers(&self, servers: &[Server]) -> rquickjs::Result<Value<'js>> {
        let list = Array::new(self.ctx.clone())?;
        for (i, server) in servers.iter().enumerate() {
            let view = Object::new(self.ctx.clone())?;
            view.set("name", server.name.as_str())?;
            view.set("online", server.online)?;
            view.set("members", server.members_only)?;
            view.set("players", server.players)?;
            view.set("capacity", server.capacity)?;
            list.set(i, view)?;
        }
        Ok(list.into_value())
    }

    fn step(&mut self, id: TaskId, resume: Resume<'js>) -> rquickjs::Result<()> {
        let Some(task) = self.tasks.get_mut(&id) else {
            return Ok(());
        };
        task.wait = Wait::Running;
        let generator = task.generator.clone();
        self.see();
        let called = match resume {
            Resume::Next(value) => self.next.call::<_, Object>((This(generator), value)),
            Resume::Throw(error) => self.throw.call::<_, Object>((This(generator), error)),
        };
        match called {
            Ok(result) if result.get::<_, bool>("done")? => {
                self.settle(id, Ok(result.get("value")?))
            }
            Ok(result) => self.block(id, result.get("value")?),
            Err(Error::Exception) => {
                let thrown = self.ctx.catch();
                self.settle(id, Err(thrown))
            }
            Err(e) => Err(e),
        }
    }

    fn block(&mut self, id: TaskId, yielded: Value<'js>) -> rquickjs::Result<()> {
        match self.op_of(yielded) {
            Some(Op::Ms(wait)) => {
                let seq = self.timer_seq;
                self.timer_seq += 1;
                self.timers.push(Reverse((Instant::now() + wait, seq, id)));
                self.wait(id, Wait::Timer(seq));
            }
            Some(Op::First(entries)) => {
                let children = entries
                    .into_iter()
                    .map(|(key, generator)| self.spawn(generator, Origin::Child(id, key)))
                    .collect();
                self.wait(id, Wait::First(children));
            }
            Some(Op::Act(_)) if !matches!(self.link, Link::Live) => {
                let message = match self.link {
                    Link::Lost => "the session is logged out",
                    Link::Live | Link::Relogging => "the session is relogging",
                };
                let error = self.aqx_error(message)?;
                self.ready.push_back((id, Resume::Throw(error)));
            }
            Some(Op::Servers) => {
                let external = self.external(id);
                let (account, wake) = (Arc::clone(&self.account), self.wake.clone());
                thread::spawn(move || {
                    let servers = External::Servers(account.servers());
                    let _ = wake.send(Wake::External(external, servers));
                });
            }
            Some(Op::Relogin(server)) => self.start_relogin(id, server)?,
            Some(Op::Logout) => {
                self.logged_out = true;
                self.session.logout();
            }
            Some(Op::Act(act)) => {
                let world = self.see();
                match act.start(&self.session, &world) {
                    Ok(Started::Sent(action, answer)) => {
                        self.actions.insert(action, id);
                        self.wait(id, Wait::Action(action, answer));
                    }
                    Ok(Started::Answered(text)) => {
                        let value = self.string(text)?;
                        self.ready.push_back((id, Resume::Next(value)));
                    }
                    Err(message) => {
                        let error = self.aqx_error(&message)?;
                        self.ready.push_back((id, Resume::Throw(error)));
                    }
                }
            }
            Some(Op::Until(predicate)) => {
                self.wait(id, Wait::Until(predicate.clone()));
                let undefined = Value::new_undefined(self.ctx.clone());
                self.offer(id, Some(predicate), undefined)?;
            }
            Some(Op::Once(event, filter)) => self.wait(id, Wait::Event(event, filter)),
            Some(Op::Packet(topic, filter)) => self.wait(id, Wait::Packet(topic, filter)),
            None => {
                let error = self
                    .type_error
                    .construct(("run an action with yield*, not yield",))?;
                self.ready.push_back((id, Resume::Throw(error)));
            }
        }
        Ok(())
    }

    fn op_of(&self, yielded: Value<'js>) -> Option<Op<'js>> {
        let op: Object = yielded.as_object()?.get(self.internals.step.clone()).ok()?;
        let filter = || op.get::<_, Option<Function>>("filter").ok();
        match op.get::<_, String>("kind").ok()?.as_str() {
            "ms" => {
                let ms: f64 = op.get("ms").ok()?;
                Duration::try_from_secs_f64(ms / 1000.0).ok().map(Op::Ms)
            }
            "first" => {
                let entries: Array = op.get("entries").ok()?;
                entries
                    .iter::<Array>()
                    .map(|entry| {
                        let entry = entry.ok()?;
                        Some((entry.get(0).ok()?, entry.get(1).ok()?))
                    })
                    .collect::<Option<Vec<_>>>()
                    .map(Op::First)
            }
            "act" => act::parse(&op).map(Op::Act),
            "until" => op.get("predicate").ok().map(Op::Until),
            "servers" => Some(Op::Servers),
            "relogin" => Some(Op::Relogin(op.get("server").ok()?)),
            "logout" => Some(Op::Logout),
            "once" => Some(Op::Once(op.get("event").ok()?, filter()?)),
            "packet" => {
                let json = match op.get::<_, String>("packetKind").ok()?.as_str() {
                    "json" => true,
                    "xt" => false,
                    _ => return None,
                };
                let topic = Topic {
                    json,
                    cmd: op.get("cmd").ok()?,
                };
                Some(Op::Packet(topic, filter()?))
            }
            _ => None,
        }
    }

    fn wait(&mut self, id: TaskId, wait: Wait<'js>) {
        if let Some(task) = self.tasks.get_mut(&id) {
            task.wait = wait;
        }
    }

    fn spawn(&mut self, generator: Object<'js>, origin: Origin) -> TaskId {
        let id = TaskId(self.ids);
        self.ids += 1;
        self.tasks.insert(
            id,
            Task {
                generator,
                origin,
                wait: Wait::Running,
            },
        );
        self.ready
            .push_back((id, Resume::Next(Value::new_undefined(self.ctx.clone()))));
        id
    }

    fn settle(
        &mut self,
        id: TaskId,
        result: Result<Value<'js>, Value<'js>>,
    ) -> rquickjs::Result<()> {
        let Some(task) = self.tasks.remove(&id) else {
            return Ok(());
        };
        let (parent, key) = match task.origin {
            Origin::Main => {
                self.end.get_or_insert(match result {
                    Ok(_) => End::Returned,
                    Err(thrown) => End::Threw(thrown),
                });
                return Ok(());
            }
            Origin::Handler(_) => {
                if let Err(thrown) = result {
                    self.end.get_or_insert(End::Threw(thrown));
                }
                return Ok(());
            }
            Origin::Child(parent, key) => (parent, key),
        };
        let siblings = match self
            .tasks
            .get_mut(&parent)
            .map(|p| std::mem::replace(&mut p.wait, Wait::Running))
        {
            Some(Wait::First(children)) => children,
            _ => return Ok(()),
        };
        for sibling in siblings.into_iter().filter(|&child| child != id) {
            self.halt(sibling)?;
        }
        let resume = match result {
            Ok(value) => {
                let won = Object::new(self.ctx.clone())?;
                won.set("winner", key)?;
                won.set("value", value)?;
                Resume::Next(won.into_value())
            }
            Err(thrown) => Resume::Throw(thrown),
        };
        self.ready.push_back((parent, resume));
        Ok(())
    }

    fn halt(&mut self, id: TaskId) -> rquickjs::Result<()> {
        let Some(task) = self.tasks.remove(&id) else {
            return Ok(());
        };
        match task.wait {
            Wait::First(children) => {
                for child in children {
                    self.halt(child)?;
                }
            }
            Wait::Action(action, _) => {
                self.actions.remove(&action);
                self.session.cancel(action);
            }
            Wait::External(external) => {
                self.externals.remove(&external);
            }
            Wait::Running
            | Wait::Timer(_)
            | Wait::Event(..)
            | Wait::Packet(..)
            | Wait::Until(_) => {}
        }
        for _ in 0..UNWIND_LIMIT {
            let undefined = Value::new_undefined(self.ctx.clone());
            match self
                .finish
                .call::<_, Object>((This(task.generator.clone()), undefined))
            {
                Ok(result) if result.get::<_, bool>("done")? => return Ok(()),
                Ok(_) => {}
                Err(Error::Exception) => {
                    let thrown = self.ctx.catch();
                    self.end.get_or_insert(End::Threw(thrown));
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        let error = self
            .type_error
            .construct(("a finally block kept yielding after aqx stopped its generator",))?;
        self.end.get_or_insert(End::Threw(error));
        Ok(())
    }

    fn next_deadline(&mut self) -> Option<Instant> {
        while let Some(Reverse((at, seq, id))) = self.timers.peek().copied() {
            if self.waits_on_timer(id, seq) {
                return Some(at);
            }
            self.timers.pop();
        }
        None
    }

    fn fire_timers(&mut self, now: Instant) {
        while let Some(Reverse((at, seq, id))) = self.timers.peek().copied() {
            if at > now {
                return;
            }
            self.timers.pop();
            if self.waits_on_timer(id, seq) {
                self.wait(id, Wait::Running);
                let undefined = Value::new_undefined(self.ctx.clone());
                self.ready.push_back((id, Resume::Next(undefined)));
            }
        }
    }

    fn waits_on_timer(&self, id: TaskId, seq: u64) -> bool {
        self.tasks
            .get(&id)
            .is_some_and(|task| matches!(task.wait, Wait::Timer(s) if s == seq))
    }
}

pub fn caught(ctx: &Ctx<'_>, error: Error) -> String {
    match Err::<(), _>(error).catch(ctx) {
        Err(CaughtError::Exception(e)) => describe(ctx, e.into_value()),
        Err(CaughtError::Value(v)) => describe(ctx, v),
        Err(CaughtError::Error(e)) => e.to_string(),
        Ok(()) => String::new(),
    }
}

fn describe<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> String {
    let Some(error) = value.as_exception() else {
        let shown =
            Coerced::<String>::from_js(ctx, value).map_or_else(|_| "a value".into(), |s| s.0);
        return format!("uncaught {shown}");
    };
    let name = error
        .get::<_, Option<String>>("name")
        .ok()
        .flatten()
        .unwrap_or_else(|| "Error".into());
    let message = error.message().unwrap_or_default();
    let stack = error.stack().unwrap_or_default();
    let frames: Vec<&str> = stack
        .lines()
        .filter(|line| !line.contains(INTERNAL_FRAME))
        .collect();
    format!("{name}: {message}\n{}", frames.join("\n"))
        .trim_end()
        .to_owned()
}
