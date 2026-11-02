mod bridge;
mod loader;
mod native;

use std::path::Path;

use aqx_core::{Event, Outcome, ScriptError, Session};
use rquickjs::function::Rest;
use rquickjs::{CatchResultExt, CaughtError, Coerced, Context, Ctx, FromJs, Function, Module, Object, Runtime, Value};

const MEMORY_LIMIT: usize = 32 << 20;
const TIMERS: &str = include_str!("timers.js");

pub fn check(path: &Path) -> Result<(), String> {
    let (name, code) = source(path)?;
    let runtime = runtime(loader::Load { checking: true })?;
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    context.with(|ctx| {
        let result = Module::declare(ctx.clone(), name, loader::halted(&code))
            .and_then(|module| module.eval())
            .and_then(|(_, promise)| promise.finish::<()>())
            .catch(&ctx);
        match result {
            Err(CaughtError::Value(v)) if v.as_string().and_then(|s| s.to_string().ok()).as_deref() == Some(loader::CHECKED) => Ok(()),
            Err(e) => Err(describe(e)),
            Ok(()) => Ok(()),
        }
    })
}

fn source(path: &Path) -> Result<(String, String), String> {
    let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.to_string_lossy().into_owned();
    let source = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
    let code = loader::compile(&name, source)?;
    Ok((name, code))
}

fn runtime(load: loader::Load) -> Result<Runtime, String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_loader(loader::Resolve, load);
    Ok(runtime)
}

pub fn run(session: &mut Session, path: &Path, args: &[(String, String)], room: Option<u32>) -> Result<Outcome, ScriptError> {
    let (name, code) = source(path).map_err(ScriptError::Load)?;
    let runtime = runtime(loader::Load { checking: false }).map_err(ScriptError::Load)?;
    let stop = session.stop_handle();
    runtime.set_interrupt_handler(Some(Box::new(move || bridge::interrupted(&stop))));
    let context = Context::full(&runtime).map_err(|e| ScriptError::Load(e.to_string()))?;
    let respawn = session.respawn_to.clone();
    session.set_hook(Some(Box::new(bridge::dispatch)));
    let outcome = context.with(|ctx| {
        let _scope = bridge::enter(&ctx, session, args, room);
        let result = install_console(&ctx)
            .and_then(|()| install_timers(&ctx))
            .and_then(|()| Module::declare(ctx.clone(), name, code))
            .and_then(|module| module.eval())
            .and_then(|(_, promise)| promise.finish::<()>())
            .catch(&ctx)
            .map_err(describe);
        bridge::finish(result)
    });
    session.set_hook(None);
    session.stop_handle().clear_interrupt();
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

fn install_timers<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<()> {
    let make: Function = ctx.eval(TIMERS)?;
    let timers: Object = make.call((Function::new(ctx.clone(), native::alarm)?,))?;
    for name in ["setTimeout", "setInterval", "clearTimeout", "clearInterval"] {
        ctx.globals().set(name, timers.get::<_, Value>(name)?)?;
    }
    bridge::set_timers(ctx, timers.get("run")?);
    Ok(())
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
