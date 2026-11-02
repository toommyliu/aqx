mod bridge;
mod loader;
mod native;
mod views;

use std::fmt;
use std::path::Path;

use aqx_core::{Event, Halt, Session};
use rquickjs::function::Rest;
use rquickjs::{CatchResultExt, CaughtError, Coerced, Context, Ctx, FromJs, Function, Module, Object, Runtime, Value};

const MEMORY_LIMIT: usize = 32 << 20;

#[derive(Debug)]
pub enum Outcome {
    Finished,
    Stopped(Option<String>),
}

#[derive(Debug)]
pub enum ScriptError {
    Load(String),
    Failed(String),
    Halted(Halt),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Load(why) | ScriptError::Failed(why) => f.write_str(why),
            ScriptError::Halted(Halt::Closed(why)) => write!(f, "{why}"),
            ScriptError::Halted(halt) => write!(f, "{halt:?}"),
        }
    }
}

pub fn run(session: &mut Session, path: &Path, args: &[(String, String)]) -> Result<Outcome, ScriptError> {
    let path = std::path::absolute(path).map_err(|e| ScriptError::Load(format!("{}: {e}", path.display())))?;
    let name = path.to_string_lossy().into_owned();
    let source = std::fs::read_to_string(&path).map_err(|e| ScriptError::Load(format!("{name}: {e}")))?;
    let code = loader::compile(&name, source).map_err(ScriptError::Load)?;
    let runtime = Runtime::new().map_err(|e| ScriptError::Load(e.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_loader(loader::Resolve, loader::Load);
    let stop = session.stop_handle();
    runtime.set_interrupt_handler(Some(Box::new(move || bridge::interrupted(&stop))));
    let context = Context::full(&runtime).map_err(|e| ScriptError::Load(e.to_string()))?;
    let respawn = session.respawn_to.clone();
    session.set_hook(Some(Box::new(bridge::dispatch)));
    let outcome = context.with(|ctx| {
        let _scope = bridge::enter(&ctx, session, args);
        let result = install_console(&ctx)
            .and_then(|()| Module::declare(ctx.clone(), name, code))
            .and_then(|module| module.eval())
            .and_then(|(_, promise)| promise.finish::<()>())
            .catch(&ctx)
            .map_err(describe);
        bridge::finish(result)
    });
    session.set_hook(None);
    session.respawn_to = respawn;
    outcome
}

fn install_console<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    for level in ["debug", "log", "info", "warn", "error"] {
        let log = move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
            let text: Vec<String> = args.0.into_iter().map(|v| show(&ctx, v)).collect();
            bridge::emit(Event::ScriptLog { level: level.into(), text: text.join(" ") });
        };
        console.set(level, Function::new(ctx.clone(), log)?.with_name(level)?)?;
    }
    ctx.globals().set("console", console)
}

fn show<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> String {
    if let Some(s) = value.as_string() {
        return s.to_string().unwrap_or_default();
    }
    if value.is_error() || value.is_undefined() || value.is_function() {
        return Coerced::<String>::from_js(ctx, value).map(|s| s.0).unwrap_or_default();
    }
    match ctx.json_stringify(value.clone()) {
        Ok(Some(json)) => json.to_string().unwrap_or_default(),
        _ => Coerced::<String>::from_js(ctx, value).map(|s| s.0).unwrap_or_default(),
    }
}

fn describe(error: CaughtError<'_>) -> String {
    match error {
        CaughtError::Exception(e) => {
            let name = e.get::<_, Option<String>>("name").ok().flatten().unwrap_or_else(|| "Error".into());
            let message = e.message().unwrap_or_default();
            let stack = e.stack().unwrap_or_default();
            let frames: Vec<&str> = stack.lines().filter(|line| !line.contains("(aqx:")).collect();
            let frames = if frames.is_empty() { stack.lines().collect() } else { frames };
            format!("{name}: {message}\n{}", frames.join("\n")).trim_end().to_string()
        }
        CaughtError::Value(v) => Coerced::<String>::from_js(v.ctx(), v.clone()).map_or_else(|_| format!("{v:?}"), |s| format!("uncaught {}", s.0)),
        CaughtError::Error(e) => e.to_string(),
    }
}
