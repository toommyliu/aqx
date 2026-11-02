#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::path::PathBuf;
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
        bot.Log($"version {bot.Version}");
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
            "version 1.4.5.0",
            "tries 5, default delay 800, exit False",
            "Question: Continue? (dismissed, as if the window were closed)",
        ]
    );
}
