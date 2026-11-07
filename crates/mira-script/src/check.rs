use std::path::Path;

use rquickjs::{Context, Module, Runtime};

use crate::{Cancel, args, engine, skua, source};

pub fn check(path: &Path) -> Result<(), String> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cs"))
    {
        return skua::SkuaScript::load(&path, |_| {}, &Cancel::default())
            .map(drop)
            .map_err(|e| e.to_string());
    }
    let name = path.to_string_lossy().into_owned();
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_loader(source::Resolve, source::Load);
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    let code = source::read(&path).map_err(|why| format!("{name}: {why}"))?;
    context.with(|ctx| {
        Module::declare(ctx.clone(), name.as_str(), code)
            .map(drop)
            .map_err(|e| engine::caught(&ctx, e))
    })?;
    let original = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
    args::schema(&name, &original).map(drop)
}
