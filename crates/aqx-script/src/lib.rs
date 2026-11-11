mod act;
mod args;
mod check;
mod engine;
mod events;
mod native;
pub mod skua;
mod source;
mod views;

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;

use aqx_client::{Handle, Phase, Server, Settings, Update};
use rquickjs::{Context, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use args::{Arg, ArgKind, Scalar};
pub use check::check;

pub const AQX_VERSION: &str = env!("CARGO_PKG_VERSION");

const MEMORY_LIMIT: usize = 32 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Log,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    Log { level: Level, text: String },
    Status(String),
    Registered(Registered),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Item {
    Id(i64),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredQuest {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reward: Option<Item>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    pub drops: Vec<Item>,
    pub quests: Vec<RegisteredQuest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edit {
    RegisterDrop(Item),
    UnregisterDrop(Item),
    RegisterQuest(RegisteredQuest),
    UnregisterQuest(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    Stopped(Option<String>),
    Restart,
    LoggedOut,
    Cancelled,
    Disconnected(String),
    Failed(String),
}

pub trait Account: Send + Sync {
    fn servers(&self) -> Result<Vec<Server>, String>;
    fn login(&self, server: &str) -> Result<Handle, String>;
    fn login_as(&self, _server: &str, _username: &str, _password: &str) -> Result<Handle, String> {
        Err("changing login identity is unavailable".into())
    }
}

pub struct Start {
    pub session: Handle,
    pub server: String,
    pub args: Map<String, Value>,
    pub account: Arc<dyn Account>,
}

pub struct Ended {
    pub outcome: Outcome,
    pub session: Handle,
    pub server: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(String);

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

impl LoadError {
    pub fn new(why: String) -> LoadError {
        LoadError(why)
    }
}

#[derive(Clone, Default)]
pub struct Cancel(Arc<Signal>);

#[derive(Default)]
struct Signal {
    cancelled: AtomicBool,
    interrupt: AtomicBool,
    paused: AtomicBool,
    wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    resumed: Mutex<Option<Box<dyn Fn() + Send>>>,
    edits: Mutex<Option<SendEdit>>,
}

type SendEdit = Box<dyn Fn(Edit) -> bool + Send>;

enum Wake {
    Update(u64, Update),
    External(u64, External),
    Cancel,
    Resume,
    Edit(Edit),
}

enum External {
    Servers(Result<Vec<Server>, String>),
    Relogged(Result<Relogged, String>),
}

struct Relogged(Option<(Handle, String)>);

impl Relogged {
    fn take(mut self) -> (Handle, String) {
        self.0.take().expect("a relogged session is taken once")
    }
}

impl Drop for Relogged {
    fn drop(&mut self) {
        if let Some((session, _)) = &self.0 {
            session.logout();
        }
    }
}

fn forward(session: &Handle, generation: u64, wake: mpsc::Sender<Wake>) -> io::Result<()> {
    let updates = session.updates();
    thread::Builder::new()
        .name("aqx-script-updates".into())
        .spawn(move || {
            for update in updates {
                if wake.send(Wake::Update(generation, update)).is_err() {
                    return;
                }
            }
        })
        .map(drop)
}

impl Cancel {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.interrupt.store(true, Ordering::SeqCst);
        if let Some(wake) = &*self.0.wake.lock().unwrap_or_else(PoisonError::into_inner) {
            wake();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    pub fn pause(&self) {
        self.0.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.0.paused.store(false, Ordering::SeqCst);
        if let Some(wake) = &*self
            .0
            .resumed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            wake();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::SeqCst)
    }

    pub fn edit(&self, edit: Edit) -> Result<(), &'static str> {
        let edits = self.0.edits.lock().unwrap_or_else(PoisonError::into_inner);
        match &*edits {
            Some(send) if send(edit) => Ok(()),
            _ => Err("the script isn't running yet"),
        }
    }

    fn on_edit(&self, send: impl Fn(Edit) -> bool + Send + 'static) {
        *self.0.edits.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(send));
    }

    fn listen(&self, wake: mpsc::Sender<Wake>) {
        let resumed = wake.clone();
        *self
            .0
            .resumed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Box::new(move || {
            let _ = resumed.send(Wake::Resume);
        }));
        let edits = wake.clone();
        self.on_edit(move |edit| edits.send(Wake::Edit(edit)).is_ok());
        let cancelled = wake.clone();
        self.on_cancel(move || {
            let _ = cancelled.send(Wake::Cancel);
        });
        if self.is_cancelled() {
            let _ = wake.send(Wake::Cancel);
        }
    }

    fn on_cancel(&self, wake: impl Fn() + Send + 'static) {
        *self.0.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(wake));
    }

    fn resume_cleanup(&self) {
        self.0.interrupt.store(false, Ordering::SeqCst);
    }
}

pub fn script_args(path: &Path) -> Result<Vec<Arg>, String> {
    let name = path.to_string_lossy();
    let source = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
    args::schema(&name, &source)
}

pub struct Script {
    context: Context,
    cancel: Cancel,
    args: Vec<Arg>,
    _runtime: Runtime,
}

impl Script {
    pub fn load(
        path: &Path,
        output: impl FnMut(Output) + 'static,
        cancel: &Cancel,
    ) -> Result<Script, LoadError> {
        let path =
            std::path::absolute(path).map_err(|e| LoadError(format!("{}: {e}", path.display())))?;
        let failed = |e: rquickjs::Error| LoadError(e.to_string());
        let runtime = Runtime::new().map_err(failed)?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        runtime.set_loader(source::Resolve, source::Load);
        let signal = Arc::clone(&cancel.0);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            signal.interrupt.load(Ordering::SeqCst)
        })));
        let context = Context::full(&runtime).map_err(failed)?;
        let name = path.to_string_lossy().into_owned();
        let source =
            std::fs::read_to_string(&path).map_err(|e| LoadError(format!("{name}: {e}")))?;
        let script = Script {
            context,
            cancel: cancel.clone(),
            args: args::schema(&name, &source).map_err(LoadError)?,
            _runtime: runtime,
        };
        script.context.with(|ctx| {
            if ctx
                .store_userdata(native::Host::new(Box::new(output)))
                .is_err()
            {
                return Err(LoadError("the script host is already in use".into()));
            }
            let loaded = engine::load(&ctx, &path)?;
            if let Some(host) = ctx.userdata::<native::Host>() {
                host.keep(loaded);
            }
            Ok(())
        })?;
        Ok(script)
    }

    pub fn args(&self) -> &[Arg] {
        &self.args
    }

    pub fn parse_args(&self, given: &[(String, String)]) -> Result<Map<String, Value>, String> {
        args::parse_all(&self.args, given)
    }

    pub fn run(self, start: Start) -> Ended {
        let (wake, inbox) = mpsc::channel();
        if let Err(e) = forward(&start.session, 0, wake.clone()) {
            return Ended {
                outcome: Outcome::Failed(format!("couldn't start the script: {e}")),
                session: start.session,
                server: start.server,
            };
        }
        self.cancel.listen(wake.clone());
        let before = start.session.snapshot().settings.clone();
        let ended = self.context.with(|ctx| {
            let loaded = ctx.userdata::<native::Host>().and_then(|host| {
                host.attach(start.session.clone());
                start.session.set_auto_relogin(host.settings().auto_relogin);
                host.set_server(&start.server);
                host.set_args(Value::Object(start.args.clone()).to_string());
                host.take_loaded()
            });
            let loaded = loaded.expect("a loaded script keeps what it loaded until it runs");
            match engine::Engine::start(&ctx, loaded, &start, wake) {
                Ok(mut engine) => {
                    let outcome = engine.run(&inbox, &self.cancel);
                    let (session, server) = engine.into_session();
                    Ended {
                        outcome,
                        session,
                        server,
                    }
                }
                Err(e) => Ended {
                    outcome: Outcome::Failed(engine::caught(&ctx, e)),
                    session: start.session.clone(),
                    server: start.server.clone(),
                },
            }
        });
        let open = !matches!(ended.session.snapshot().phase, Phase::Closed { .. });
        if open {
            let same = ended.session.same_session(&start.session);
            restore_after_run(
                &ended.session,
                if same { before } else { Settings::default() },
            );
        }
        ended
    }
}

fn restore_after_run(session: &Handle, settings: Settings) {
    restore(
        session,
        Settings {
            auto_relogin: true,
            ..settings
        },
    );
}

fn restore(session: &Handle, settings: Settings) {
    session.set_auto_relogin(settings.auto_relogin);
    session.set_counter_hold(settings.counter_hold);
    session.set_aggro(settings.aggro);
    session.set_anti_afk(settings.anti_afk);
    session.set_room(settings.room);
}

impl Drop for Script {
    fn drop(&mut self) {
        self.context.with(|ctx| {
            let _ = ctx.remove_userdata::<native::Host>();
        });
    }
}
