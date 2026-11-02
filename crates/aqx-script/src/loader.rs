use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module, Result};
use swc_common::errors::Handler;
use swc_common::sync::Lrc;
use swc_common::{GLOBALS, Globals, SourceMap};
use swc_ts_fast_strip::{Mode, Options};

use crate::native::Native;

const API: &str = include_str!("aqx.js");

pub struct Resolve;

impl Resolver for Resolve {
    fn resolve<'js>(&mut self, _: &Ctx<'js>, base: &str, name: &str, _: Option<ImportAttributes<'js>>) -> Result<String> {
        match name {
            "aqx" => Ok(name.into()),
            "aqx:native" if base == "aqx" => Ok(name.into()),
            _ if name.starts_with("./") || name.starts_with("../") => {
                let dir = Path::new(base).parent().unwrap_or(Path::new(""));
                Ok(normalize(&dir.join(name)).to_string_lossy().into_owned())
            }
            _ => Err(Error::new_resolving_message(base, name, "only `aqx` and relative imports are supported")),
        }
    }
}

pub struct Load;

impl Loader for Load {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str, _: Option<ImportAttributes<'js>>) -> Result<Module<'js, Declared>> {
        match name {
            "aqx" => Module::declare(ctx.clone(), name, API),
            "aqx:native" => Module::declare_def::<Native, _>(ctx.clone(), name),
            _ => {
                let source = std::fs::read_to_string(name).map_err(|e| Error::new_loading_message(name, e.to_string()))?;
                let code = compile(name, source).map_err(|e| Error::new_loading_message(name, e))?;
                Module::declare(ctx.clone(), name, code)
            }
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            part => out.push(part),
        }
    }
    out
}

#[derive(Clone, Default)]
struct Diagnostics(Arc<Mutex<Vec<u8>>>);

impl Write for Diagnostics {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().map_err(|_| std::io::ErrorKind::Other)?.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn compile(name: &str, source: String) -> std::result::Result<String, String> {
    if !name.ends_with(".ts") {
        return Ok(source);
    }
    let cm: Lrc<SourceMap> = Default::default();
    let diagnostics = Diagnostics::default();
    let handler = Handler::with_emitter_writer(Box::new(diagnostics.clone()), Some(cm.clone()));
    let options = Options { module: Some(true), filename: Some(name.into()), mode: Mode::StripOnly, ..Default::default() };
    let stripped = GLOBALS.set(&Globals::default(), || swc_ts_fast_strip::operate(&cm, &handler, source, options));
    stripped.map(|out| out.code).map_err(|e| {
        let shown = diagnostics.0.lock().map(|d| String::from_utf8_lossy(&d).trim().to_string()).unwrap_or_default();
        if shown.is_empty() { e.to_string() } else { shown }
    })
}
