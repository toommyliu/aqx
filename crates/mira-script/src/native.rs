use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use mira_client::{Aggro, CounterHold, Handle, ItemId, MonMapId, QuestId, Settings, World};
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Constructor, Ctx, Function, JsLifetime, Object, Result, Symbol, Value};

use crate::engine::Loaded;
use crate::{AutoRelogin, Level, Output, views};

pub struct Internals<'js> {
    pub step: Symbol<'js>,
    pub mira_error: Constructor<'js>,
    pub dispatch_event: Function<'js>,
    pub dispatch_packet: Function<'js>,
}

pub struct Host<'js> {
    output: RefCell<Box<dyn FnMut(Output)>>,
    internals: RefCell<Option<Internals<'js>>>,
    loaded: RefCell<Option<Loaded<'js>>>,
    stop: RefCell<Option<Option<String>>>,
    world: RefCell<Option<Arc<World>>>,
    version: Cell<u64>,
    session: RefCell<Option<Handle>>,
    settings: RefCell<Settings>,
    spawned: RefCell<Vec<(u64, Object<'js>)>>,
    halted: RefCell<Vec<u64>>,
    packet_handlers: Cell<u32>,
    server: RefCell<String>,
    args: RefCell<String>,
    auto_relogin: RefCell<Option<AutoRelogin>>,
    restart: Cell<bool>,
}

unsafe impl<'js> JsLifetime<'js> for Host<'js> {
    type Changed<'to> = Host<'to>;
}

impl<'js> Host<'js> {
    pub fn new(output: Box<dyn FnMut(Output)>) -> Host<'js> {
        Host {
            output: RefCell::new(output),
            internals: RefCell::new(None),
            loaded: RefCell::new(None),
            stop: RefCell::new(None),
            world: RefCell::new(None),
            version: Cell::new(0),
            session: RefCell::new(None),
            settings: RefCell::new(Settings::default()),
            spawned: RefCell::new(Vec::new()),
            halted: RefCell::new(Vec::new()),
            packet_handlers: Cell::new(0),
            server: RefCell::new(String::new()),
            args: RefCell::new("{}".into()),
            auto_relogin: RefCell::new(None),
            restart: Cell::new(false),
        }
    }

    pub fn set_server(&self, server: &str) {
        self.server.replace(server.to_owned());
    }

    pub fn set_args(&self, json: String) {
        self.args.replace(json);
    }

    pub fn auto_relogin(&self) -> Option<AutoRelogin> {
        self.auto_relogin.borrow().clone()
    }

    pub fn restart_requested(&self) -> bool {
        self.restart.get()
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    fn session(&self) -> Option<Handle> {
        self.session.borrow().clone()
    }

    pub fn take_spawned(&self) -> Vec<(u64, Object<'js>)> {
        self.spawned.take()
    }

    pub fn take_halted(&self) -> Vec<u64> {
        self.halted.take()
    }

    pub fn watches_packets(&self) -> bool {
        self.packet_handlers.get() > 0
    }

    pub fn attach(&self, session: Handle) {
        self.session.replace(Some(session));
    }

    pub fn world(&self) -> Option<Arc<World>> {
        self.world.borrow().clone()
    }

    pub fn take_internals(&self) -> Option<Internals<'js>> {
        self.internals.take()
    }

    pub fn keep(&self, loaded: Loaded<'js>) {
        self.loaded.replace(Some(loaded));
    }

    pub fn take_loaded(&self) -> Option<Loaded<'js>> {
        self.loaded.take()
    }

    pub fn stop_requested(&self) -> Option<Option<String>> {
        self.stop.borrow().clone()
    }

    pub fn see(&self, world: Arc<World>) {
        self.world.replace(Some(world));
        self.version.set(self.version.get() + 1);
    }

    fn emit(&self, output: Output) {
        (self.output.borrow_mut())(output);
    }
}

fn host<'a, 'js>(ctx: &'a Ctx<'js>) -> rquickjs::runtime::UserDataGuard<'a, Host<'js>> {
    ctx.userdata::<Host<'js>>()
        .expect("a script stores its host before it loads")
}

pub struct Native;

const NAMES: [&str; 23] = [
    "setCounterHold",
    "setAggro",
    "setAntiAfk",
    "setRoom",
    "setAutoRelogin",
    "server",
    "restart",
    "args",
    "spawn",
    "halt",
    "watchPackets",
    "quest",
    "hasRoom",
    "init",
    "log",
    "warn",
    "error",
    "status",
    "stop",
    "version",
    "view",
    "untarget",
    "setSpawnPoint",
];

impl ModuleDef for Native {
    fn declare<'js>(decl: &Declarations<'js>) -> Result<()> {
        for name in NAMES {
            decl.declare(name)?;
        }
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let log =
            |level| move |ctx: Ctx<'js>, text: String| host(&ctx).emit(Output::Log { level, text });
        exports.export(
            "init",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>,
                 step: Symbol<'js>,
                 mira_error: Constructor<'js>,
                 dispatch_event: Function<'js>,
                 dispatch_packet: Function<'js>| {
                    host(&ctx).internals.replace(Some(Internals {
                        step,
                        mira_error,
                        dispatch_event,
                        dispatch_packet,
                    }));
                },
            )?,
        )?;
        exports.export("log", Function::new(ctx.clone(), log(Level::Log))?)?;
        exports.export("warn", Function::new(ctx.clone(), log(Level::Warn))?)?;
        exports.export("error", Function::new(ctx.clone(), log(Level::Error))?)?;
        exports.export(
            "status",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
                host(&ctx).emit(Output::Status(text))
            })?,
        )?;
        exports.export(
            "stop",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, reason: Option<String>| {
                host(&ctx).stop.borrow_mut().get_or_insert(reason);
            })?,
        )?;
        exports.export(
            "version",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| host(&ctx).version.get() as f64)?,
        )?;
        exports.export(
            "view",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, name: String| -> Result<Value<'js>> {
                    let world = host(&ctx).world();
                    match world {
                        Some(world) => views::view(&ctx, &world, &name),
                        None => Ok(Value::new_null(ctx.clone())),
                    }
                },
            )?,
        )?;
        exports.export(
            "setCounterHold",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, hold: String| {
                let hold = match hold.as_str() {
                    "all" => CounterHold::All,
                    "attacks" => CounterHold::Attacks,
                    _ => CounterHold::Off,
                };
                let host = host(&ctx);
                host.settings.borrow_mut().counter_hold = hold;
                host.session().map(|s| s.set_counter_hold(hold)).is_some()
            })?,
        )?;
        exports.export(
            "setAggro",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, mode: String, ids: Vec<f64>| {
                let aggro = match mode.as_str() {
                    "cell" => Some(Aggro::Cell),
                    "only" => Some(Aggro::Only(
                        ids.into_iter()
                            .map(|id| MonMapId(id as i64))
                            .collect::<HashSet<_>>(),
                    )),
                    _ => None,
                };
                let host = host(&ctx);
                host.settings.borrow_mut().aggro = aggro.clone();
                host.session().map(|s| s.set_aggro(aggro)).is_some()
            })?,
        )?;
        exports.export(
            "setAntiAfk",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, on: bool| {
                let host = host(&ctx);
                host.settings.borrow_mut().anti_afk = on;
                host.session().map(|s| s.set_anti_afk(on)).is_some()
            })?,
        )?;
        exports.export(
            "setRoom",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, room: Option<u32>| {
                let host = host(&ctx);
                host.settings.borrow_mut().room = room;
                host.session().map(|s| s.set_room(room)).is_some()
            })?,
        )?;
        exports.export(
            "setAutoRelogin",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, delay: Option<f64>, attempts: u32, server: Option<String>| {
                    let policy = delay.map(|ms| AutoRelogin {
                        delay: Duration::from_secs_f64(ms / 1000.0),
                        attempts,
                        server,
                    });
                    host(&ctx).auto_relogin.replace(policy);
                },
            )?,
        )?;
        exports.export(
            "server",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| {
                host(&ctx).server.borrow().clone()
            })?,
        )?;
        exports.export(
            "restart",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| host(&ctx).restart.set(true))?,
        )?;
        exports.export(
            "args",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| -> Result<Option<Value<'js>>> {
                let host = host(&ctx);
                if host.session.borrow().is_none() {
                    return Ok(None);
                }
                let json = host.args.borrow().clone();
                ctx.json_parse(json).map(Some)
            })?,
        )?;
        exports.export(
            "spawn",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, tag: f64, task: Object<'js>| {
                host(&ctx).spawned.borrow_mut().push((tag as u64, task));
            })?,
        )?;
        exports.export(
            "halt",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, tag: f64| {
                host(&ctx).halted.borrow_mut().push(tag as u64);
            })?,
        )?;
        exports.export(
            "watchPackets",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, count: u32| {
                host(&ctx).packet_handlers.set(count);
            })?,
        )?;
        exports.export(
            "quest",
            Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, id: f64| -> Result<Value<'js>> {
                    match host(&ctx).world() {
                        Some(world) => views::quest(&ctx, &world, QuestId(id as i64)),
                        None => Ok(Value::new_null(ctx.clone())),
                    }
                },
            )?,
        )?;
        exports.export(
            "hasRoom",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, id: f64, pool: String| {
                let pool = views::parse_pool(&pool);
                let world = host(&ctx).world();
                world
                    .zip(pool)
                    .is_some_and(|(world, pool)| world.has_room(ItemId(id as i64), pool))
            })?,
        )?;
        exports.export(
            "setSpawnPoint",
            Function::new(ctx.clone(), |ctx: Ctx<'js>, cell: String, pad: String| {
                host(&ctx)
                    .session()
                    .map(|session| session.set_spawn_point(&cell, &pad))
                    .is_some()
            })?,
        )?;
        exports.export(
            "untarget",
            Function::new(ctx.clone(), |ctx: Ctx<'js>| {
                host(&ctx)
                    .session()
                    .map(|session| session.untarget())
                    .is_some()
            })?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use swc_common::sync::Lrc;
    use swc_common::{FileName, SourceMap};
    use swc_ecma_ast::{Decl, Expr, ModuleItem, Stmt, TsTypeElement};
    use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax};

    use super::NAMES;

    #[test]
    fn the_native_module_exports_what_mira_ts_declares() {
        let map: Lrc<SourceMap> = Lrc::default();
        let file = map.new_source_file(
            Lrc::new(FileName::Custom("mira.ts".into())),
            include_str!("mira.ts").to_owned(),
        );
        let module = Parser::new(
            Syntax::Typescript(TsSyntax::default()),
            StringInput::from(&*file),
            None,
        )
        .parse_module()
        .unwrap();
        let native = module
            .body
            .iter()
            .find_map(|item| match item {
                ModuleItem::Stmt(Stmt::Decl(Decl::TsInterface(i))) if &*i.id.sym == "Native" => {
                    Some(i)
                }
                _ => None,
            })
            .expect("mira.ts declares interface Native");
        let declared: BTreeSet<String> = native
            .body
            .body
            .iter()
            .filter_map(|member| match member {
                TsTypeElement::TsMethodSignature(method) => match &*method.key {
                    Expr::Ident(name) => Some(name.sym.to_string()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let exported: BTreeSet<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(declared, exported);
    }
}
