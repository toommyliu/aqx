use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module, Result};
use swc_common::errors::Handler;
use swc_common::sync::Lrc;
use swc_common::{GLOBALS, Globals, SourceMap};
use swc_ts_fast_strip::{Mode, Options};

use crate::native::Native;

const AQX: &str = "aqx";
const NATIVE: &str = "aqx:native";
const AQX_TS: &str = include_str!("aqx.ts");

pub struct Resolve;

impl Resolver for Resolve {
    fn resolve<'js>(
        &mut self,
        _: &Ctx<'js>,
        base: &str,
        name: &str,
        _: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        match name {
            AQX => Ok(name.to_owned()),
            NATIVE if base == AQX => Ok(name.to_owned()),
            _ if name.starts_with("./") || name.starts_with("../") => {
                let dir = Path::new(base).parent().unwrap_or(Path::new(""));
                Ok(normalize(&dir.join(name)).to_string_lossy().into_owned())
            }
            _ => Err(Error::new_resolving_message(
                base,
                name,
                "a script can import only \"aqx\" and relative paths",
            )),
        }
    }
}

pub struct Load;

impl Loader for Load {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        match name {
            AQX => {
                let code = strip_types(name, AQX_TS.to_owned())
                    .map_err(|why| Error::new_loading_message(name, why))?;
                Module::declare(ctx.clone(), name, code)
            }
            NATIVE => Module::declare_def::<Native, _>(ctx.clone(), name),
            _ => {
                let code =
                    read(Path::new(name)).map_err(|why| Error::new_loading_message(name, why))?;
                Module::declare(ctx.clone(), name, code)
            }
        }
    }
}

pub fn read(path: &Path) -> std::result::Result<String, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if path.extension().is_some_and(|ext| ext == "ts") {
        strip_types(&path.to_string_lossy(), source)
    } else {
        Ok(source)
    }
}

fn strip_types(name: &str, source: String) -> std::result::Result<String, String> {
    let map: Lrc<SourceMap> = Lrc::default();
    let diagnostics = Diagnostics::default();
    let handler = Handler::with_emitter_writer(Box::new(diagnostics.clone()), Some(map.clone()));
    let options = Options {
        module: Some(true),
        filename: Some(name.to_owned()),
        mode: Mode::StripOnly,
        ..Options::default()
    };
    GLOBALS
        .set(&Globals::default(), || {
            swc_ts_fast_strip::operate(&map, &handler, source, options)
        })
        .map(|out| out.code)
        .map_err(|e| {
            let shown = diagnostics.text();
            if shown.is_empty() {
                e.to_string()
            } else {
                shown
            }
        })
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

impl Diagnostics {
    fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).trim().to_owned()
    }
}

impl Write for Diagnostics {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
