use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use aqx_core::{ActError, Event, Goal, Halt, Monster, Predicate, Session, Sink, StopHandle, Target, World};
use rquickjs::{Coerced, Ctx, Exception, Function, Persistent, Value, qjs};

use crate::{Outcome, ScriptError, views};

const WATCHDOG: Duration = Duration::from_secs(5);

thread_local! {
    static SESSION: Cell<*mut Session> = const { Cell::new(ptr::null_mut()) };
    static WORLD: Cell<*const World> = const { Cell::new(ptr::null()) };
    static CTX: Cell<Option<NonNull<qjs::JSContext>>> = const { Cell::new(None) };
    static LAST_CALL: Cell<Option<Instant>> = const { Cell::new(None) };
    static ENDING: Cell<bool> = const { Cell::new(false) };
    static STUCK: Cell<bool> = const { Cell::new(false) };
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    make_error: Option<Persistent<Function<'static>>>,
    dispatch: Option<Persistent<Function<'static>>>,
    timers: Option<Persistent<Function<'static>>>,
    listening: HashSet<String>,
    pending: Option<Persistent<Value<'static>>>,
    end: Option<End>,
    args: Vec<(String, String)>,
    room: Option<u32>,
    sink: Sink,
    stop: StopHandle,
}

enum End {
    Stopped(Option<String>),
    Halted(Halt),
}

pub struct Scope;

pub fn enter(ctx: &Ctx<'_>, session: &mut Session, args: &[(String, String)], room: Option<u32>) -> Scope {
    let state = State {
        make_error: None,
        dispatch: None,
        timers: None,
        listening: HashSet::new(),
        pending: None,
        end: None,
        args: args.to_vec(),
        room,
        sink: session.sink(),
        stop: session.stop_handle(),
    };
    SESSION.set(session);
    WORLD.set(ptr::null());
    CTX.set(Some(ctx.as_raw()));
    ENDING.set(false);
    STUCK.set(false);
    touch();
    STATE.set(Some(state));
    Scope
}

impl Drop for Scope {
    fn drop(&mut self) {
        SESSION.set(ptr::null_mut());
        WORLD.set(ptr::null());
        CTX.set(None);
        LAST_CALL.set(None);
        STATE.set(None);
    }
}

pub fn finish(result: Result<(), String>) -> Result<Outcome, ScriptError> {
    let (end, stopped) = with_state(|s| (s.end.take(), s.stop.is_stopped()));
    match end {
        Some(End::Stopped(reason)) => Ok(Outcome::Stopped(reason)),
        Some(End::Halted(halt)) => Err(ScriptError::Halted(halt)),
        None if stopped => Err(ScriptError::Halted(Halt::Stopped)),
        None if STUCK.get() => Err(ScriptError::Failed(format!("the script ran {} s without calling aqx", WATCHDOG.as_secs()))),
        None => result.map(|()| Outcome::Finished).map_err(ScriptError::Failed),
    }
}

pub fn interrupted(stop: &StopHandle) -> bool {
    if stop.is_stopped() || ENDING.get() {
        return true;
    }
    let stuck = LAST_CALL.get().is_some_and(|at| at.elapsed() > WATCHDOG);
    if stuck {
        STUCK.set(true);
    }
    stuck
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with_borrow_mut(|s| f(s.as_mut().expect("a script is running")))
}

fn touch() {
    LAST_CALL.set(Some(Instant::now()));
}

pub fn init<'js>(ctx: &Ctx<'js>, make_error: Function<'js>, dispatch: Function<'js>) {
    let (make_error, dispatch) = (Persistent::save(ctx, make_error), Persistent::save(ctx, dispatch));
    with_state(|s| {
        s.make_error = Some(make_error);
        s.dispatch = Some(dispatch);
    });
}

pub fn set_timers<'js>(ctx: &Ctx<'js>, run: Function<'js>) {
    let run = Persistent::save(ctx, run);
    with_state(|s| s.timers = Some(run));
}

pub fn listen(name: String, on: bool) {
    with_state(|s| {
        if on {
            s.listening.insert(name);
        } else {
            s.listening.remove(&name);
        }
    });
}

pub fn room() -> Option<u32> {
    with_state(|s| s.room)
}

pub fn args() -> Vec<(String, String)> {
    with_state(|s| s.args.clone())
}

pub fn emit(event: Event) {
    let sink = with_state(|s| s.sink.clone());
    sink(&event);
}

pub fn stop<'js>(ctx: &Ctx<'js>, reason: Option<String>) -> rquickjs::Error {
    let message = reason.clone().unwrap_or_else(|| "the script stopped".into());
    end(End::Stopped(reason));
    aqx_error(ctx, "cancelled", &message)
}

fn end(end: End) {
    ENDING.set(true);
    with_state(|s| {
        s.end.get_or_insert(end);
        s.stop.interrupt();
    });
}

fn live<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<()> {
    if ENDING.get() {
        return Err(aqx_error(ctx, "cancelled", "the script is ending"));
    }
    Ok(())
}

pub fn can_block() -> bool {
    !SESSION.get().is_null()
}

pub fn read<'js, T>(ctx: &Ctx<'js>, f: impl FnOnce(&World) -> T) -> rquickjs::Result<T> {
    live(ctx)?;
    let world = WORLD.get();
    if !world.is_null() {
        return Ok(f(unsafe { &*world }));
    }
    let session = SESSION.get();
    if session.is_null() {
        return Err(Exception::throw_message(ctx, "the game state isn't available here"));
    }
    Ok(f(unsafe { &(*session).world }))
}

pub fn with_session<'js, T>(ctx: &Ctx<'js>, what: &str, f: impl FnOnce(&mut Session) -> T) -> rquickjs::Result<T> {
    live(ctx)?;
    let session = SESSION.get();
    if session.is_null() {
        return Err(Exception::throw_message(ctx, &format!("{what} can't be called inside a predicate")));
    }
    Ok(f(unsafe { &mut *session }))
}

pub fn blocking<'js, T>(ctx: &Ctx<'js>, what: &str, f: impl FnOnce(&mut Session) -> Result<T, ActError>) -> rquickjs::Result<T> {
    let result = with_session(ctx, what, |s| {
        touch();
        let result = f(s);
        if !matches!(result, Err(ActError::Halted(Halt::Stopped | Halt::Closed(_)))) && !failed() {
            s.run_handlers();
        }
        result
    })?;
    touch();
    if let Some(pending) = with_state(|s| s.pending.take()) {
        with_state(|s| s.stop.clear_interrupt());
        let value = pending.restore(ctx)?;
        return Err(ctx.throw(value));
    }
    result.map_err(|e| act_error(ctx, e))
}

fn act_error<'js>(ctx: &Ctx<'js>, e: ActError) -> rquickjs::Error {
    let message = e.to_string();
    let code = match e {
        ActError::Halted(Halt::Timeout) | ActError::Timeout(_) => "timeout",
        ActError::NotFound(_) => "not_found",
        ActError::Rejected(_) => "rejected",
        ActError::Halted(Halt::Interrupted | Halt::GoalMet) => "cancelled",
        ActError::Halted(halt @ (Halt::Stopped | Halt::Closed(_))) => {
            end(End::Halted(halt));
            "cancelled"
        }
    };
    aqx_error(ctx, code, &message)
}

pub fn aqx_error<'js>(ctx: &Ctx<'js>, code: &str, message: &str) -> rquickjs::Error {
    let make = with_state(|s| s.make_error.clone()).and_then(|f| f.restore(ctx).ok());
    match make.map(|f| f.call::<_, Value>((code, message))) {
        Some(Ok(error)) => ctx.throw(error),
        Some(Err(e)) => e,
        None => Exception::throw_message(ctx, message),
    }
}

fn fail<'js>(ctx: &Ctx<'js>, e: rquickjs::Error) {
    let value = if e.is_exception() {
        ctx.catch()
    } else {
        Exception::from_message(ctx.clone(), &e.to_string()).map_or_else(|_| Value::new_undefined(ctx.clone()), |x| x.into_value())
    };
    let value = Persistent::save(ctx, value);
    with_state(|s| {
        s.pending.get_or_insert(value);
        s.stop.interrupt();
    });
}

fn failed() -> bool {
    ENDING.get() || with_state(|s| s.pending.is_some())
}

fn inside<T>(world: &World, f: impl FnOnce() -> T) -> T {
    let saved = (SESSION.replace(ptr::null_mut()), WORLD.replace(world));
    touch();
    let result = f();
    SESSION.set(saved.0);
    WORLD.set(saved.1);
    result
}

pub fn check<'js>(ctx: &Ctx<'js>, world: &World, call: impl FnOnce() -> rquickjs::Result<Coerced<bool>>) -> bool {
    if failed() {
        return false;
    }
    match inside(world, call) {
        Ok(Coerced(yes)) => yes,
        Err(e) => {
            fail(ctx, e);
            false
        }
    }
}

pub fn predicate<'js>(ctx: &Ctx<'js>, f: Function<'js>) -> Predicate<'js> {
    let ctx = ctx.clone();
    Box::new(move |world| check(&ctx, world, || f.call(())))
}

pub fn lasting_target<'js>(ctx: &Ctx<'js>, f: Function<'js>) -> Target {
    let f = Persistent::save(ctx, f);
    Box::new(move |world: &World, monster: &Monster| {
        let Some(raw) = CTX.get() else { return false };
        let ctx = unsafe { Ctx::from_raw(raw) };
        match f.clone().restore(&ctx) {
            Ok(f) => check(&ctx, world, || f.call((ctx.json_parse(views::monster(monster).to_string())?,))),
            Err(_) => false,
        }
    })
}

pub fn goal<'js>(ctx: &Ctx<'js>, f: Function<'js>) -> Goal {
    let f = Persistent::save(ctx, f);
    Box::new(move |world| {
        let Some(raw) = CTX.get() else { return false };
        let ctx = unsafe { Ctx::from_raw(raw) };
        match f.clone().restore(&ctx) {
            Ok(f) => check(&ctx, world, || f.call(())),
            Err(_) => false,
        }
    })
}

pub fn dispatch(session: &mut Session) {
    let session: *mut Session = session;
    let (events, queued) = unsafe { ((*session).take_script_events(), (*session).events_queued()) };
    let first = queued - events.len() as u64;
    let Some(raw) = CTX.get() else { return };
    let ctx = unsafe { Ctx::from_raw(raw) };
    let Some(handler) = with_state(|s| s.dispatch.clone()).and_then(|f| f.restore(&ctx).ok()) else { return };
    let saved = (SESSION.replace(session), WORLD.replace(ptr::null()));
    for (seq, event) in (first..).zip(&events) {
        if failed() {
            break;
        }
        let called = if let Event::Alarm = event {
            let Some(run) = with_state(|s| s.timers.clone()).and_then(|f| f.restore(&ctx).ok()) else { continue };
            touch();
            run.call::<_, ()>(())
        } else {
            let Some(name) = views::event_name(event) else { continue };
            if name != "packet" && !with_state(|s| s.listening.contains(name)) {
                continue;
            }
            let payload = views::event(unsafe { &(*session).world }, event);
            touch();
            ctx.json_parse(payload.to_string()).and_then(|payload| handler.call::<_, ()>((name, payload, seq as f64)))
        };
        if let Err(e) = called {
            fail(&ctx, e);
        }
    }
    SESSION.set(saved.0);
    WORLD.set(saved.1);
}
