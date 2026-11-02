#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use aqx_core::{Event, Outcome, Sink};
use server::{FARM, Fake};

const HELLO: &str = r#"
using Skua.Core.Interfaces;

public class Hello
{
    public void ScriptMain(IScriptInterface bot)
    {
        bot.Log($"version {bot.Version?.ToString() ?? "unknown"}");
        bot.Options.JoinMapTries = 5;
        bot.Log($"tries {bot.Options.JoinMapTries}, default delay {bot.Options.ActionDelay}, exit {bot.ShouldExit}");
        bot.ShowMessageBox("Continue?", "Question", true);
        bot.Sleep(10);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn the_skua_host_runs_a_script_against_the_session() {
    let host = std::env::var_os("AQX_SKUA").expect("AQX_SKUA names the aqx-skua executable");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-host");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("Hello.cs");
    std::fs::write(&script, HELLO).unwrap();

    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink: Sink = {
        let logged = logged.clone();
        Rc::new(move |event: &Event| {
            if let Event::ScriptLog { text, .. } = event {
                logged.lock().unwrap().push(text.clone());
            }
        })
    };
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let mut command = Command::new(host);
    command.arg("serve").arg(&script);
    let result = aqx_remote::run(&mut fake.session, command);

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert_eq!(
        logged,
        [
            "version unknown",
            "tries 5, default delay 800, exit False",
            "Question: Continue? (dismissed, as if the window were closed)",
        ]
    );
}

fn host(script: &Path, home: &Path) -> Command {
    let host = std::env::var_os("AQX_SKUA").expect("AQX_SKUA names the aqx-skua executable");
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skua/vendor/Scripts");
    let mut command = Command::new(host);
    command.env("AQX_SKUA_DATA", home.join("Skua")).arg("serve").arg(script).arg("--scripts").arg(scripts);
    command
}

fn logging() -> (Sink, Arc<Mutex<Vec<String>>>) {
    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink: Sink = {
        let logged = logged.clone();
        Rc::new(move |event: &Event| match event {
            Event::ScriptLog { level, text } => logged.lock().unwrap().push(format!("{level}: {text}")),
            Event::Status { text } => logged.lock().unwrap().push(format!("status: {text}")),
            _ => {}
        })
    };
    (sink, logged)
}

const SET_OPTIONS: &str = r#"
//cs_include Scripts/CoreBots.cs
using Skua.Core.Interfaces;

public class SetOptionsOnly
{
    public IScriptInterface Bot => IScriptInterface.Instance;
    public CoreBots Core => CoreBots.Instance;

    public void ScriptMain(IScriptInterface bot)
    {
        Core.SetOptions();
        Core.Logger("options set");
        Core.SetOptions(false);
    }
}
"#;

#[test]
#[ignore = "needs the .NET host: build skua/Host and point AQX_SKUA at it"]
fn core_bots_sets_its_options_up_and_down() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("skua-set-options");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let script = dir.join("SetOptionsOnly.cs");
    std::fs::write(&script, SET_OPTIONS).unwrap();

    let (sink, logged) = logging();
    let mut fake = Fake::start_with(FARM, |_, _| {}, sink);
    let result = aqx_remote::run(&mut fake.session, host(&script, &home));

    let logged = logged.lock().unwrap().clone();
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}\n{logged:#?}");
    assert!(logged.iter().any(|l| l.contains("options set")), "{logged:#?}");
    if std::env::var_os("AQX_SKUA_LOG").is_some() {
        eprintln!("{logged:#?}");
    }
}
