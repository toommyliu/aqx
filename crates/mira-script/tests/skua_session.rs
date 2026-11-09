mod support;
use mira_client::{Handle, Server, testing::Harness};
use mira_protocol::{AreaId, MonMapId, packet};
use mira_script::{Account, Cancel, Outcome};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::Fake;

#[derive(Default)]
struct LoginAccount {
    sessions: Mutex<Vec<Harness>>,
    attempts: Mutex<Vec<String>>,
    sent: Arc<Mutex<Vec<String>>>,
    farm: bool,
}
impl Account for LoginAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        Ok([
            ("Offline", false, false, 0, 100),
            ("Test", true, false, 0, 100),
            ("Full", true, false, 100, 100),
            ("Members", true, true, 0, 100),
            ("Preferred", true, false, 0, 100),
            ("Fallback", true, false, 0, 100),
        ]
        .into_iter()
        .map(|(name, online, members_only, players, capacity)| Server {
            name: name.into(),
            host: "localhost".into(),
            port: 5588,
            online,
            members_only,
            players,
            capacity,
        })
        .collect())
    }
    fn login(&self, server: &str) -> Result<Handle, String> {
        self.attempts.lock().unwrap().push(server.into());
        if server == "Preferred" {
            return Err("unavailable".into());
        }
        let (sent, farm) = (self.sent.clone(), self.farm);
        let session = Harness::start(Default::default(), move |listener| {
            let mut peer = mira_client::testing::Peer::accept(&listener);
            if farm {
                peer.serve_farm();
            } else {
                peer.serve_to_ready();
            }
            loop {
                let message = peer.recv();
                let logout = message == packet::logout();
                sent.lock().unwrap().push(message);
                if logout {
                    break;
                }
            }
        });
        let handle = session.session.clone();
        self.sessions.lock().unwrap().push(session);
        Ok(handle)
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn automatic_relogin_waits_for_the_account_and_never_logs_in_on_its_own() {
    let account = Arc::new(LoginAccount::default());
    let (outcome, logs, _) = Fake::start(|msg, wire| {
        if msg == "disconnect" {
            wire.hang_up();
        }
    })
    .run_account(
        r#"
using Skua.Core.Interfaces;
public class Offline {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"run relogin {bot.Options.AutoRelogin}");
        bot.Options.AutoRelogin = true;
        bot.Events.ReloginTriggered += kicked => bot.Log($"relogin kicked {kicked}");
        bot.Send.Packet("disconnect");
        bot.Sleep(20000);
        bot.Log("old script continued");
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        account.clone(),
        |_| {},
    );
    assert!(
        !matches!(outcome, Outcome::Failed(_)),
        "{outcome:?} {logs:?}"
    );
    assert_eq!(
        logs.iter().filter(|s| s.starts_with("run ")).count(),
        1,
        "{logs:?}"
    );
    assert!(
        logs.contains(&"run relogin False".into()),
        "an account without auto_relogin: {logs:?}"
    );
    assert!(logs.contains(&"relogin kicked False".into()), "{logs:?}");
    assert!(!logs.contains(&"old script continued".into()));
    assert!(account.attempts.lock().unwrap().is_empty());
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_reconnecting_session_stops_the_script_and_starts_it_again_once_back() {
    let account = Arc::new(LoginAccount::default());
    let (outcome, logs, _) = Fake::reconnecting(|msg, wire| {
        if msg == "disconnect" {
            wire.hang_up();
        }
    })
    .run_account(
        r#"
using System;
using Skua.Core.Interfaces;
public class Reconnected {
    public void ScriptMain(IScriptInterface bot) {
        bool again = Environment.GetEnvironmentVariable("MIRA_TEST_RUN") == "1";
        bot.Log($"run again {again} on {bot.Servers.LastName} relogin {bot.Options.AutoRelogin} to {bot.Options.ReloginServer}");
        if (again) {
            bool fired = false;
            bot.Handlers.RegisterOnce(1, _ => fired = true);
            bot.Wait.ForTrue(() => fired, 2);
            bot.Log($"restarted handler {fired}");
            return;
        }
        Environment.SetEnvironmentVariable("MIRA_TEST_RUN", "1");
        bot.Handlers.RegisterOnce(10000, _ => bot.Log("stale handler"));
        bot.Events.ReloginTriggered += kicked => bot.Log($"relogin kicked {kicked}");
        bot.Events.ScriptStopping += _ => { bot.Log("stopping"); return true; };
        bot.Send.Packet("disconnect");
        for (;;) { bot.Map.Jump("r2", "Left"); bot.Sleep(100); }
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        account.clone(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(
        logs.contains(&"run again False on Test relogin True to Test".into()),
        "{logs:?}"
    );
    assert!(logs.contains(&"stopping".into()), "{logs:?}");
    assert!(logs.contains(&"relogin kicked False".into()), "{logs:?}");
    assert!(
        logs.contains(&"run again True on Test relogin True to Test".into()),
        "{logs:?}"
    );
    assert!(logs.contains(&"restarted handler True".into()), "{logs:?}");
    assert!(!logs.contains(&"stale handler".into()), "{logs:?}");
    assert!(
        account.attempts.lock().unwrap().is_empty(),
        "the session logs back in by itself"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn turning_auto_relogin_off_leaves_the_account_logged_out() {
    let (outcome, logs, sent) = Fake::reconnecting(|msg, wire| {
        if msg == "disconnect" {
            wire.hang_up();
        }
    })
    .run_account(
        r#"
using Skua.Core.Interfaces;
public class StayOut {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log("run");
        bot.Options.AutoRelogin = false;
        bot.Send.Packet("disconnect");
        bot.Sleep(20000);
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        Arc::new(LoginAccount::default()),
        |_| {},
    );
    assert!(
        !matches!(outcome, Outcome::Failed(_)),
        "{outcome:?} {logs:?}"
    );
    assert_eq!(logs.iter().filter(|s| *s == "run").count(), 1, "{logs:?}");
    let joins = sent.iter().filter(|m| m.contains("%moveToCell%")).count();
    assert_eq!(joins, 1, "the account never logged back in: {sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_door_packet_moves_the_player_once_the_server_answers() {
    let (outcome, logs, sent) = Fake::start(|message, wire| {
        if message.contains("%mtcid%") {
            wire.send("%xt%mtcid%-1%5%");
        }
    })
    .run(
        r#"
using Skua.Core.Interfaces;
public class Door {
    public void ScriptMain(IScriptInterface bot) {
        _ = bot.Map.Cells;
        bot.Send.Packet($"%xt%zm%mtcid%{bot.Map.RoomID}%5%");
        bot.Wait.ForCellChange("r2");
        bot.Log($"cell {bot.Player.Cell} {bot.Player.Pad}");
    }
}
"#,
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"cell r2 Left".into()), "{logs:?}");
    assert!(
        !sent.iter().any(|m| m.contains("%moveToCell%5%r2%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn an_action_after_the_session_ends_fails_instead_of_hanging() {
    let cancel = Cancel::default();
    let deadline = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(10));
        deadline.cancel();
    });
    let (outcome, logs, _) = Fake::start(|_, _| {}).run_with(
        r#"
using Skua.Core.Interfaces;
public class AfterLogout {
    public void ScriptMain(IScriptInterface bot) {
        bot.Servers.Logout();
        bot.Wait.ForTrue(() => !bot.Servers.IsConnected, 50);
        bot.Sleep(300);
        try { bot.Player.Rest(); bot.Log("rest returned"); }
        catch (OperationCanceledException) { bot.Log("rest failed"); }
    }
}
"#,
        &cancel,
        Default::default(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"rest failed".into()), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_relogin_clears_bank_loaded_and_drops_and_keeps_aggro_all_on() {
    let account = Arc::new(LoginAccount {
        farm: true,
        ..LoginAccount::default()
    });
    let (outcome, logs, _) = Fake::start(|msg, wire| {
        if msg == "drop" {
            wire.object(r#"{"cmd":"dropItem","items":{"5":{"ItemID":5,"sName":"Dust","iQty":1,"iStk":10}}}"#);
        }
    })
    .run_account(
        r#"
using Skua.Core.Interfaces;
public class Relogged {
    public void ScriptMain(IScriptInterface bot) {
        bot.Bank.Load();
        _ = bot.Drops.CurrentDrops;
        bot.Send.Packet("drop");
        bot.Wait.ForTrue(() => bot.Drops.Exists("Dust"), 50);
        bot.Log($"before relogin {bot.Bank.Loaded} {bot.Drops.Exists("Dust")}");
        bot.Options.AggroAllMonsters = true;
        bot.Events.CellChanged += (map, cell, pad) => Thread.Sleep(1500);
        bot.Map.Jump("r2", "Left");
        bot.Log($"relogged {bot.Servers.Relogin("Fallback")}");
        bot.Log($"after relogin {bot.Bank.Loaded} {bot.Drops.Exists("Dust")} {bot.Options.AggroAllMonsters}");
        bot.Sleep(2500);
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        account.clone(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "before relogin True True",
        "relogged True",
        "after relogin False False True",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    for session in account.sessions.lock().unwrap().drain(..) {
        session.finish();
    }
    let sent = account.sent.lock().unwrap();
    let all = packet::aggro_mon(AreaId(43), &[MonMapId(1), MonMapId(2), MonMapId(3)]);
    assert!(sent.contains(&all), "{sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_relogin_keeps_aggro_monsters_pulling_the_cell() {
    let account = Arc::new(LoginAccount {
        farm: true,
        ..LoginAccount::default()
    });
    let (outcome, logs, _) = Fake::start(|_, _| {}).run_account(
        r#"
using Skua.Core.Interfaces;
public class Relogged {
    public void ScriptMain(IScriptInterface bot) {
        bot.Options.AggroMonsters = true;
        bot.Log($"relogged {bot.Servers.Relogin("Fallback")}");
        bot.Sleep(1500);
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        account.clone(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"relogged True".into()), "{logs:?}");
    for session in account.sessions.lock().unwrap().drain(..) {
        session.finish();
    }
    let sent = account.sent.lock().unwrap();
    let cell = packet::aggro_mon(AreaId(43), &[MonMapId(1), MonMapId(2)]);
    assert!(sent.contains(&cell), "{sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn automatic_relogin_restarts_past_a_stopping_handler_that_never_returns() {
    let cancel = Cancel::default();
    let deadline = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        deadline.cancel();
    });
    let (outcome, logs, _) = Fake::reconnecting(|msg, wire| {
        if msg == "disconnect" {
            wire.hang_up();
        }
    })
    .run_account(
        r#"
using System;
using System.Threading;
using Skua.Core.Interfaces;
public class Stuck {
    public void ScriptMain(IScriptInterface bot) {
        bool again = Environment.GetEnvironmentVariable("MIRA_TEST_RUN") == "1";
        bot.Log($"run again {again}");
        if (again) return;
        Environment.SetEnvironmentVariable("MIRA_TEST_RUN", "1");
        bot.Events.ScriptStopping += _ => { Thread.Sleep(Timeout.Infinite); return true; };
        bot.Send.Packet("disconnect");
        for (;;) { bot.Map.Jump("r2", "Left"); bot.Sleep(100); }
    }
}
"#,
        &cancel,
        Default::default(),
        Arc::new(LoginAccount::default()),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"run again True".into()), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn automatic_relogin_drops_the_previous_runs_flash_callbacks() {
    let (outcome, logs, _) = Fake::reconnecting(|msg, wire| {
        if msg == "disconnect" {
            wire.hang_up();
        }
    })
    .run_account(
        r#"
using System;
using Skua.Core.Interfaces;
public class Callbacks {
    public void ScriptMain(IScriptInterface bot) {
        string run = Environment.GetEnvironmentVariable("MIRA_TEST_RUN") ?? "first";
        bot.Flash.FlashCall += (name, args) => { if (name == "packet" && args[0].ToString()!.Contains("probe")) bot.Log($"{run} saw probe"); };
        if (run == "second") {
            bot.Send.Packet("%xt%zm%cmd%1%probe%");
            bot.Sleep(300);
            return;
        }
        Environment.SetEnvironmentVariable("MIRA_TEST_RUN", "second");
        bot.Send.Packet("disconnect");
        for (;;) { bot.Map.Jump("r2", "Left"); bot.Sleep(100); }
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        Arc::new(LoginAccount::default()),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    let probes: Vec<&String> = logs.iter().filter(|l| l.ends_with("saw probe")).collect();
    assert_eq!(probes, ["second saw probe"], "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn join_packet_is_nonblocking_and_jump_honors_client_only_auto_correct_and_delay() {
    let started = Instant::now();
    let (outcome, logs, sent) = Fake::start(|_, _| {}).run(
        r#"
using Skua.Core.Interfaces;
public class Movement {
    public void ScriptMain(IScriptInterface bot) {
        bot.Options.ActionDelay = 100;
        bot.Map.JoinPacket("other-42", "r2", "Left");
        bot.Log("join returned");
        bot.Map.Jump("r1", "Left", false, true);
        bot.Log($"local cell {bot.Player.Cell}");
        var clock = System.Diagnostics.Stopwatch.StartNew();
        bot.Map.Jump("r2", "Right", false);
        bot.Log($"delayed {clock.ElapsedMilliseconds >= 100}");
        bot.Map.Jump("r2", "Bottom");
        bot.Log($"corrected to {bot.Player.Pad}");
    }
}
"#,
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(logs.contains(&"join returned".into()));
    assert!(logs.contains(&"local cell r1".into()), "{logs:?}");
    assert!(logs.contains(&"delayed True".into()), "{logs:?}");
    assert!(logs.contains(&"corrected to Left".into()), "{logs:?}");
    assert!(
        sent.iter().any(|m| m.ends_with("%moveToCell%5%r2%Right%")),
        "{sent:?}"
    );
    assert!(
        sent.iter().any(|m| m.ends_with("%other-42%r2%Left%")),
        "{sent:?}"
    );
    assert!(
        !sent.iter().any(|m| m.contains("%moveToCell%5%r1%")),
        "{sent:?}"
    );
}

#[test]
#[ignore = "run with npm run skua:test"]
fn packets_go_out_while_another_call_waits_on_the_server() {
    let mut asked = false;
    let (outcome, logs, sent) = Fake::start(move |message, wire| {
        if message.contains("%getQuests%") {
            asked = true;
        } else if asked && message.contains("%afk%") {
            wire.object(r#"{"cmd":"getQuests","quests":{"19":{"QuestID":19,"sName":"Repeat","bOnce":false,"iSlot":-1,"turnin":[],"oItems":{},"oRewards":{},"reward":[]}}}"#);
        } else if message.contains("%acceptQuest%5%19%") {
            wire.object(r#"{"cmd":"acceptQuest","QuestID":19,"bSuccess":1}"#);
        }
    })
    .run(
        r#"
using System.Diagnostics;
using System.Threading.Tasks;
using Skua.Core.Interfaces;
public class Packets {
    public void ScriptMain(IScriptInterface bot) {
        Task<bool> accept = Task.Run(() => bot.Quests.Accept(19));
        bot.Sleep(200);
        Stopwatch clock = Stopwatch.StartNew();
        bot.Send.ClientPacket("%xt%server%-1%hello%");
        bot.Send.Packet("%xt%zm%afk%1%false%");
        bot.Log($"packets {(clock.ElapsedMilliseconds < 500 ? "went at once" : "waited")}");
        bot.Log($"accepted {accept.Result}");
    }
}
"#,
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in ["packets went at once", "accepted True"] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
    let at = |wanted: &str| sent.iter().position(|m| m.contains(wanted));
    assert!(at("%getQuests%") < at("%afk%"), "{sent:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn a_client_packet_level_up_shows_in_the_avatar_data_but_not_the_player_level() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(
        r#"
using Skua.Core.Interfaces;
public class FakeLevel {
    public void ScriptMain(IScriptInterface bot) {
        bot.Send.ClientPacket(
            "{\"t\":\"xt\",\"b\":{\"r\":-1,\"o\":{\"cmd\":\"levelUp\",\"intExpToLevel\":\"0\",\"intLevel\":100}}}",
            type: "json"
        );
        bot.Wait.ForTrue(() => bot.Flash.GetGameObject("world.myAvatar.objData.intLevel") == "100", 20);
        bot.Log($"level {bot.Player.Level}, shown {bot.Flash.GetGameObject("world.myAvatar.objData.intLevel")}");
    }
}
"#,
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"level 50, shown 100".into()), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn account_tags_reject_unknown_accounts_and_lite_keys_share_typed_values() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(r#"
using Skua.Core.Interfaces;
public class Preferences {
    public void ScriptMain(IScriptInterface bot) {
        bot.Log($"unknown {bot.Accounts.AddTags("missing", "tag")} {bot.Accounts.AddTag(bot.Player.Username, "tag")} {bot.Accounts.GetAllAccounts().Count}");
        bot.Lite.Set("bReaccept", true);
        bot.Log($"reaccept {bot.Lite.ReacceptQuest}");
        bot.Lite.ReacceptQuest = false;
        bot.Log($"raw reaccept {bot.Lite.Get<bool>("bReaccept")}");
        bot.Options.ActionDelay = 123;
        bot.Log($"dictionary {bot.Options.OptionDictionary["ActionDelay"]()}");
        bot.Options.ResetToDefault();
        bot.Log($"default delay {bot.Options.ActionDelay}");
    }
}
"#);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    for expected in [
        "unknown False False 0",
        "reaccept True",
        "raw reaccept False",
        "dictionary 123",
        "default delay 800",
    ] {
        assert!(logs.contains(&expected.into()), "{logs:?}");
    }
}

struct SlowAccount;
impl Account for SlowAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        LoginAccount::default().servers()
    }
    fn login(&self, _: &str) -> Result<Handle, String> {
        std::thread::sleep(Duration::from_millis(500));
        Err("unavailable".into())
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn login_timeout_and_token_cancel_bound_inflight_relogin() {
    let (outcome, logs, _) = Fake::start(|_, _| {}).run_account(r#"
using Skua.Core.Interfaces;
public class Deadlines {
    public void ScriptMain(IScriptInterface bot) {
        bot.Options.LoginTimeout = 50;
        var clock = System.Diagnostics.Stopwatch.StartNew();
        bot.Log($"timed out {!bot.Servers.Relogin("Fallback") && clock.ElapsedMilliseconds < 300}");
        bot.Options.LoginTimeout = 30000;
        bot.Options.ReloginTries = 1;
        using var token = new CancellationTokenSource(50);
        clock.Restart();
        bot.Log($"cancelled {!bot.Servers.EnsureRelogin("Fallback", token.Token).GetAwaiter().GetResult() && clock.ElapsedMilliseconds < 300}");
    }
}
"#, &Cancel::default(), Default::default(), Arc::new(SlowAccount), |_| {});
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"timed out True".into()), "{logs:?}");
    assert!(logs.contains(&"cancelled True".into()), "{logs:?}");
}

#[test]
#[ignore = "run with npm run skua:test"]
fn map_join_uses_private_defaults_sentinel_and_requested_destination() {
    let (outcome, logs, sent) = Fake::start(|msg, wire| {
        if msg.contains("%tfer%") {
            let fields: Vec<_> = msg.split('%').collect();
            let map = fields[7];
            let base = map.split('-').next().unwrap();
            wire.object(&support::FARM.replace("farm-1", map).replace(
                "\"strMapName\":\"farm\"",
                &format!("\"strMapName\":\"{base}\""),
            ));
            wire.send("<msg t='sys'><body action='joinOK' r='0'><uLs r='5'></uLs></body></msg>");
            wire.send(&format!("%xt%server%-1%You joined \"{map}\"!%"));
        }
    })
    .run(
        r#"
using Skua.Core.Interfaces;
public class PrivateMaps {
    public void ScriptMain(IScriptInterface bot) {
        bot.Options.ActionDelay = 0;
        bot.Options.PrivateRooms = true;
        bot.Map.Join("other", "r2", "Left", autoCorrect: false);
        bot.Options.PrivateRooms = false;
        bot.Map.Join("third-1e9", "r1", "Right", autoCorrect: false);
        bot.Map.Join("other-42", "Enter", "Spawn", autoCorrect: false);
        bot.Map.Join("other-43", ignoreCheck: true);
        bot.Options.JoinMapTries = 1;
        bot.Map.Join("unused");
        bot.Log("joined");
    }
}
"#,
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    let transfers: Vec<_> = sent.iter().filter(|m| m.contains("%tfer%")).collect();
    assert_eq!(transfers.len(), 3, "{transfers:?}");
    for suffix in [
        "%other-100000%r2%Left%",
        "%third-100000%r1%Right%",
        "%other-42%Enter%Spawn%",
    ] {
        assert!(
            transfers.iter().any(|m| m.ends_with(suffix)),
            "{transfers:?}"
        );
    }
}

struct AlternateAccount(Mutex<bool>);
impl Account for AlternateAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        LoginAccount::default().servers()
    }
    fn login(&self, _: &str) -> Result<Handle, String> {
        panic!("expected alternate login")
    }
    fn login_as(&self, server: &str, username: &str, password: &str) -> Result<Handle, String> {
        assert_eq!(
            (server, username, password),
            ("Fallback", "player2", "placeholder")
        );
        *self.0.lock().unwrap() = true;
        Err("unavailable".into())
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn set_login_info_reaches_the_rust_account_without_being_logged() {
    let account = Arc::new(AlternateAccount(Mutex::new(false)));
    let (outcome, logs, _) = Fake::start(|_, _| {}).run_account(
        r#"
using Skua.Core.Interfaces;
public class LoginIdentity {
    public void ScriptMain(IScriptInterface bot) {
        bot.Servers.SetLoginInfo("player2", "placeholder");
        bot.Log($"connected {bot.Servers.Relogin("Fallback")}");
    }
}
"#,
        &Cancel::default(),
        Default::default(),
        account.clone(),
        |_| {},
    );
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(*account.0.lock().unwrap());
    assert!(!logs.iter().any(|s| s.contains("placeholder")));
}

#[derive(Default)]
struct ServerAccount(std::sync::atomic::AtomicUsize);
impl Account for ServerAccount {
    fn servers(&self) -> Result<Vec<Server>, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        LoginAccount::default().servers()
    }
    fn login(&self, _: &str) -> Result<Handle, String> {
        panic!("unexpected login")
    }
}

#[test]
#[ignore = "run with npm run skua:test"]
fn server_lists_cache_success_refresh_explicitly_and_tolerate_http_failure() {
    let account = Arc::new(ServerAccount::default());
    let (outcome, logs, _) = Fake::start(|_, _| {}).run_account(r#"
using Skua.Core.Interfaces;
public class ServerCache {
    public void ScriptMain(IScriptInterface bot) {
        int changed = 0;
        bot.Servers.PropertyChanged += (_, e) => { if (e.PropertyName == "CachedServers") changed++; };
        bot.Log($"initial cache {bot.Servers.CachedServers.Count}");
        bot.Servers.GetServers().GetAwaiter().GetResult();
        bot.Servers.GetServers().GetAwaiter().GetResult();
        bot.Servers.GetServers(true).GetAwaiter().GetResult();
        bot.Servers.LastName = "Preferred";
        bot.Log($"refreshes {changed} last {bot.Servers.LastName}");
    }
}
"#, &Cancel::default(), Default::default(), account.clone(), |_| {});
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert_eq!(account.0.load(std::sync::atomic::Ordering::Relaxed), 2);
    assert!(logs.contains(&"initial cache 0".into()), "{logs:?}");
    assert!(
        logs.contains(&"refreshes 2 last Preferred".into()),
        "{logs:?}"
    );
    let (outcome, logs, _) = Fake::start(|_, _| {}).run(r#"
using Skua.Core.Interfaces;
public class UnavailableServers {
    public void ScriptMain(IScriptInterface bot) => bot.Log($"unavailable {bot.Servers.GetServers().GetAwaiter().GetResult().Count}");
}
"#);
    assert_eq!(outcome, Outcome::Finished, "{logs:?}");
    assert!(logs.contains(&"unavailable 0".into()), "{logs:?}");
}
