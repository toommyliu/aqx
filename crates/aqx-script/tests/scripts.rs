#[path = "../../aqx-core/tests/server/mod.rs"]
mod server;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use aqx_core::{Outcome, ScriptError};
use aqx_script::{Arg, check, run, script_args};
use server::{FARM, Fake, Wire};

fn write(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, source) in files {
        let file = dir.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    dir.join(files[0].0)
}

fn run_script(fake: &mut Fake, test: &str, source: &str) -> Result<Outcome, ScriptError> {
    run(&mut fake.session, &write(test, &[("main.ts", source)]), &[], None)
}

fn sent(fake: Fake) -> Vec<String> {
    fake.finish().into_iter().map(|(_, m)| m).collect()
}

fn killed_on(cast: usize) -> impl FnMut(&str, &mut Wire) + Send {
    let mut casts = 0;
    move |msg, wire| {
        if !msg.contains("%gar%") {
            return;
        }
        casts += 1;
        if casts >= cast {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
            wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
        } else {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":500,"intState":2}}}"#);
        }
    }
}

#[track_caller]
fn finished(result: Result<Outcome, ScriptError>) {
    assert!(matches!(result, Ok(Outcome::Finished)), "{result:?}");
}

#[track_caller]
fn failed_with(result: Result<Outcome, ScriptError>, text: &str) {
    assert!(matches!(&result, Err(ScriptError::Failed(why)) if why.contains(text)), "wanted a failure mentioning {text:?}, got {result:?}");
}

#[test]
fn handlers_run_while_a_kill_blocks() {
    let mut fake = Fake::start(FARM, killed_on(2));
    let result = run_script(
        &mut fake,
        "handlers",
        r#"
        import { combat, events, map, player } from "aqx";
        const kills: string[] = [];
        events.on("kill", (m) => kills.push(`${m.name} in ${player.cell}`));
        combat.kill("Treeant");
        if (kills.join() !== "Treeant in r2") throw new Error(`kills: ${kills.join()}`);
        if (map.find("treeant")[0].alive) throw new Error("the Treeant is alive");
        "#,
    );
    finished(result);
    assert!(sent(fake).iter().any(|m| m.contains("%gar%")));
}

#[test]
fn a_throwing_predicate_aborts_the_action_with_its_error() {
    let mut fake = Fake::start(FARM, killed_on(1));
    let result = run_script(
        &mut fake,
        "throwing",
        r#"
        import { combat } from "aqx";
        combat.kill("Treeant", { cell: "r2", hold: () => { throw new Error("boom"); } });
        "#,
    );
    failed_with(result, "boom");
    assert!(!sent(fake).iter().any(|m| m.contains("%gar%")), "cast after the predicate threw");
}

#[test]
fn predicates_cannot_block() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = run_script(
        &mut fake,
        "blocking",
        r#"
        import { map, wait } from "aqx";
        wait.until(() => { map.jump("r1"); return true; }, 1000);
        "#,
    );
    failed_with(result, "map.jump can't be called inside a predicate");
    assert!(!sent(fake).iter().any(|m| m.contains("%moveToCell%5%r1%")));
}

#[test]
fn stop_ends_the_script_even_when_caught() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = run_script(
        &mut fake,
        "stop",
        r#"
        import { map, script } from "aqx";
        try { script.stop("enough"); } catch {}
        map.jump("r1");
        "#,
    );
    assert!(matches!(&result, Ok(Outcome::Stopped(Some(why))) if why == "enough"), "{result:?}");
    assert!(!sent(fake).iter().any(|m| m.contains("%moveToCell%5%r1%")));
}

#[test]
fn args_take_their_type_from_the_defaults() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let source = r#"
        import { script } from "aqx";
        const { n, name, fast } = script.args({ n: 1, name: "x", fast: false });
        if (n !== 3 || name !== "a=b" || fast !== true) throw new Error(JSON.stringify({ n, name, fast }));
    "#;
    let path = write("args", &[("main.ts", source)]);
    let given = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    finished(run(&mut fake.session, &path, &given(&[("n", "3"), ("name", "a=b"), ("fast", "true")]), None));
    failed_with(run(&mut fake.session, &path, &given(&[("m", "3")]), None), "unknown --arg m; this script takes n, name, fast");
    failed_with(run(&mut fake.session, &path, &given(&[("n", "three")]), None), "--arg n=three isn't a number");
    fake.finish();
}

#[test]
fn a_constructor_in_place_of_a_default_makes_the_arg_required() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let source = r#"
        import { script } from "aqx";
        const { target, n, class: cls } = script.args({ target: String, n: Number, class: "" });
        if (target !== "Bob Smith" || n !== 2 || cls !== "") throw new Error(JSON.stringify({ target, n, cls }));
    "#;
    let path = write("required", &[("main.ts", source)]);
    let given = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
    finished(run(&mut fake.session, &path, &given(&[("target", "Bob Smith"), ("n", "2")]), None));
    failed_with(run(&mut fake.session, &path, &given(&[("n", "2")]), None), "missing --arg target");
    failed_with(run(&mut fake.session, &path, &[], None), "missing --arg target, --arg n");
    fake.finish();
}

#[test]
fn script_args_reads_each_arg_and_its_default_without_a_game() {
    let main = r#"
        import { player, script } from "./lib.ts";
        console.log("starting");
        const args = script.args({ target: String, class: "", rounds: Number, fast: false, n: 2 });
        while (true) player.cell;
    "#;
    let lib = r#"export * from "aqx";"#;
    let arg = |key: &str, default: Option<&str>| Arg { key: key.into(), default: default.map(String::from) };
    let declared = [arg("target", None), arg("class", Some("")), arg("rounds", None), arg("fast", Some("false")), arg("n", Some("2"))];
    assert_eq!(script_args(&write("describe", &[("main.ts", main), ("lib.ts", lib)])), declared);
    let after_a_game_call = r#"import { map, script } from "aqx"; map.jump("r1"); script.args({ target: String });"#;
    assert_eq!(script_args(&write("describe-late", &[("main.ts", after_a_game_call)])), [arg("target", None)]);
    let endless = r#"import { script } from "aqx"; while (true) {} script.args({ target: String });"#;
    for (test, source) in [("describe-endless", endless), ("describe-broken", "this is not js")] {
        assert!(script_args(&write(test, &[("main.ts", source)])).is_empty(), "{test}");
    }
}

#[test]
fn typescript_imports_relative_modules_and_nothing_else() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let main = r#"
        import type { Fight } from "aqx";
        import { double } from "./lib.ts";
        const fight: Fight = { skills: [1] };
        if (double(21) !== 42 || fight.skills?.length !== 1) throw new Error("wrong");
    "#;
    let lib = "export interface Unused { a: string }\nexport const double = (n: number): number => n * 2;\n";
    finished(run(&mut fake.session, &write("imports", &[("main.ts", main), ("lib.ts", lib)]), &[], None));
    failed_with(run_script(&mut fake, "fs", r#"import fs from "fs";"#), "only `aqx` and relative imports are supported");
    failed_with(run_script(&mut fake, "native", r#"import * as n from "aqx:native";"#), "only `aqx` and relative imports are supported");
    fake.finish();
}

#[test]
fn failed_actions_throw_aqx_errors_with_a_code() {
    let mut fake = Fake::start(FARM, |_, _| {});
    fake.session.world.set_bank(Vec::new());
    let result = run_script(
        &mut fake,
        "codes",
        r#"
        import { AqxError, bank, events } from "aqx";
        const code = (action: () => void) => {
            try { action(); } catch (e) { return e instanceof AqxError ? e.code : `not an AqxError: ${e}`; }
            return "no error";
        };
        const codes = [code(() => bank.withdraw("Nothing")), code(() => events.once("death", { timeout: 50 }))];
        if (codes.join() !== "not_found,timeout") throw new Error(codes.join());
        "#,
    );
    finished(result);
    fake.finish();
}

const QUEST: &str = r#"{"cmd":"getQuests","quests":{"11":{"QuestID":11,"sName":"Twilly's New Staff","bOnce":false,"iSlot":-1,"iValue":0,"bUpg":false,"iLvl":0,
    "turnin":[{"ItemID":87,"iQty":1}],"oItems":{"87":{"ItemID":87,"sName":"Treeant Branch"}},"reward":[]}}}"#;

fn quest_giver(msg: &str, wire: &mut Wire) {
    if msg.contains("%getQuests%5%11%") {
        wire.object(QUEST);
    } else if msg.contains("%acceptQuest%5%11%") {
        wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#);
        wire.object(r#"{"cmd":"addItems","items":{"87":{"ItemID":87,"sName":"Treeant Branch","bTemp":true,"iQty":1}}}"#);
    } else if msg.contains("%tryQuestComplete%5%11%") {
        wire.object(r#"{"cmd":"turnIn","sItems":"87:1"}"#);
        wire.object(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":11,"sName":"Twilly's New Staff","rewardObj":{}}"#);
    }
}

#[test]
fn registered_quests_complete_and_restart_in_the_background() {
    let mut fake = Fake::start(FARM, quest_giver);
    let result = run_script(
        &mut fake,
        "register",
        r#"
        import { events, quests, wait } from "aqx";
        const seen: string[] = [];
        events.on("quest", (q) => seen.push(q.change));
        quests.register(11);
        if (!wait.until(() => seen.join() === "accepted,completed,accepted", 5000)) throw new Error(seen.join());
        quests.unregister(11);
        "#,
    );
    finished(result);
}

#[test]
fn a_handler_does_not_take_the_reply_an_action_waits_for() {
    let mut fake = Fake::start(FARM, quest_giver);
    let result = run_script(
        &mut fake,
        "reply",
        r#"
        import { events, quests, wait } from "aqx";
        let done = false;
        events.on("item", (item) => {
            if (item.qty > 0 && !done) {
                quests.complete(11);
                done = true;
            }
        });
        quests.accept(11);
        if (!wait.until(() => done, 5000)) throw new Error("the handler never completed the quest");
        "#,
    );
    finished(result);
}

fn treeants_for_quest_11() -> impl FnMut(&str, &mut Wire) + Send {
    let mut respawns = Instant::now();
    move |msg, wire| {
        if msg.contains("%getQuests%5%11%") {
            wire.object(QUEST);
        } else if msg.contains("%acceptQuest%5%11%") {
            wire.object(r#"{"cmd":"acceptQuest","bSuccess":1,"QuestID":11,"msg":"success"}"#);
        } else if msg.contains("%tryQuestComplete%5%11%") {
            wire.object(r#"{"cmd":"turnIn","sItems":"87:1"}"#);
            wire.object(r#"{"cmd":"ccqr","bSuccess":1,"QuestID":11,"sName":"Twilly's New Staff","rewardObj":{}}"#);
        } else if msg.contains("%gar%") && Instant::now() >= respawns {
            respawns = Instant::now() + Duration::from_millis(300);
            wire.object(r#"{"cmd":"addItems","items":{"87":{"ItemID":87,"sName":"Treeant Branch","bTemp":true,"iQty":1}}}"#);
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
            wire.later(Duration::from_millis(300), "<respawn>", r#"{"cmd":"mtls","id":2,"o":{"intHP":900,"intState":1}}"#.into());
        }
    }
}

#[test]
fn farm_returns_once_its_goal_is_met_even_when_a_registered_quest_takes_the_items() {
    let mut fake = Fake::start(FARM, treeants_for_quest_11());
    fake.session.world.me.as_mut().unwrap().char_id = Some(1);
    let result = run_script(
        &mut fake,
        "farm-goal",
        r#"
        import { combat, events, quests, wait } from "aqx";
        let completed = 0;
        events.on("quest", (q) => { if (q.change === "completed") completed++; });
        quests.register(11);
        for (let i = 1; i <= 2; i++) {
            combat.farm(87, 1, { target: "Treeant", timeout: 8000 });
            if (!wait.until(() => completed >= i, 5000)) throw new Error(`completed ${completed} times, wanted ${i}`);
        }
        quests.unregister(11);
        if (completed !== 2) throw new Error(`completed ${completed} times`);
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn kill_until_stops_as_soon_as_it_holds() {
    let mut fake = Fake::start(FARM, treeants_for_quest_11());
    let result = run_script(
        &mut fake,
        "kill-until",
        r#"
        import { combat, events, inventory, quests, wait } from "aqx";
        let completed = 0;
        events.on("quest", (q) => { if (q.change === "completed") completed++; });
        quests.register(11);
        combat.kill("Treeant", { until: () => inventory.temp.has(87), timeout: 8000 });
        if (!wait.until(() => completed === 1, 3000)) throw new Error(`completed ${completed} times`);
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn strict_rotations_carry_on_across_kills() {
    let mut respawns = Instant::now();
    let mut fake = Fake::start(FARM, move |msg, wire| {
        if msg.contains("%gar%") && !msg.contains("%aa>") && Instant::now() >= respawns {
            respawns = Instant::now() + Duration::from_millis(300);
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":0,"intState":0}}}"#);
            wire.object(r#"{"cmd":"addGoldExp","intGold":10,"intExp":5,"typ":"m","id":2}"#);
            wire.later(Duration::from_millis(300), "<respawn>", r#"{"cmd":"mtls","id":2,"o":{"intHP":900,"intState":1}}"#.into());
        }
    });
    let result = run_script(
        &mut fake,
        "strict",
        r#"
        import { combat, events } from "aqx";
        let kills = 0;
        events.on("kill", () => kills++);
        combat.kill("Treeant", { skills: [1, 3], strict: true, until: () => kills >= 2, timeout: 10000 });
        "#,
    );
    finished(result);
    let class: Vec<String> = sent(fake).iter().filter(|m| m.contains("%gar%") && !m.contains("%aa>")).map(|m| m.split('%').nth(6).unwrap_or("").to_string()).collect();
    assert!(class.len() >= 2 && class[0].starts_with("a1>") && class[1].starts_with("a3>"), "{class:?}");
}

#[test]
fn stopping_inside_a_handler_ends_the_call_it_ran_under() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%5%r1%") {
            wire.later(Duration::from_millis(800), "<item>", r#"{"cmd":"addItems","items":{"87":{"ItemID":87,"sName":"Treeant Branch","bTemp":true,"iQty":1}}}"#.into());
        }
    });
    let started = Instant::now();
    let result = run_script(
        &mut fake,
        "stop-handler",
        r#"
        import { events, map, script, wait } from "aqx";
        events.on("item", () => { try { script.stop("from a handler"); } catch {} });
        map.jump("r1");
        wait.until(() => false, 4000);
        "#,
    );
    assert!(matches!(&result, Ok(Outcome::Stopped(Some(why))) if why == "from a handler"), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(3), "took {:?}", started.elapsed());
    fake.finish();
}

#[test]
fn a_call_delivers_its_events_before_it_returns() {
    let mut fake = Fake::start(FARM, quest_giver);
    let result = run_script(
        &mut fake,
        "delivered",
        r#"
        import { events, quests } from "aqx";
        let accepted = 0;
        events.on("quest", (q) => { if (q.change === "accepted") accepted++; });
        quests.accept(11);
        if (accepted !== 1) throw new Error(`handlers saw ${accepted} accepts before accept returned`);
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn once_ignores_events_queued_before_it_was_called() {
    let mut fake = Fake::start(FARM, quest_giver);
    let result = run_script(
        &mut fake,
        "once",
        r#"
        import { AqxError, events, quests } from "aqx";
        let completing = false;
        events.on("quest", (q) => {
            if (q.change === "accepted" && !completing) {
                completing = true;
                quests.complete(11);
            }
        });
        quests.accept(11);
        try {
            const e = events.once("quest", { timeout: 1000 });
            throw new Error(`got an old event: ${JSON.stringify(e)}`);
        } catch (e) {
            if (!(e instanceof AqxError) || e.code !== "timeout") throw e;
        }
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn relative_imports_climb_out_of_a_relative_script_path() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let main = write("climb", &[("scripts/main.ts", r#"import { answer } from "../shared/util.ts"; if (answer !== 42) throw new Error("wrong module");"#), ("shared/util.ts", "export const answer: number = 42;")]);
    let cwd = std::env::current_dir().unwrap();
    let ups = cwd.components().count() - 1;
    let relative = PathBuf::from("../".repeat(ups)).join(main.strip_prefix("/").unwrap());
    finished(run(&mut fake.session, &relative, &[], None));
    fake.finish();
}

#[test]
fn a_busy_script_is_stopped() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let started = Instant::now();
    failed_with(run_script(&mut fake, "busy", r#"import { player } from "aqx"; while (player.hp >= 0) {}"#), "without calling aqx");
    assert!(started.elapsed() < Duration::from_secs(8));
    fake.finish();
}

#[test]
fn timers_fire_while_the_script_waits() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = run_script(
        &mut fake,
        "timers",
        r#"
        import { wait } from "aqx";
        const fired: string[] = [];
        setTimeout((tag: string) => fired.push(tag), 50, "once");
        const cancelled = setTimeout(() => fired.push("cancelled"), 60);
        clearTimeout(cancelled);
        let ticks = 0;
        const every = setInterval(() => { if (++ticks === 3) clearInterval(every); }, 40);
        wait.ms(400);
        if (fired.join() !== "once" || ticks !== 3) throw new Error(`${fired.join()} ${ticks}`);
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn packets_reach_handlers_for_their_cmd_and_answer_requests() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg == "%xt%zm%hi%1%" {
            wire.send("%xt%hi%-1%");
            wire.object(r#"{"cmd":"event","args":{"zoneSet":"A"}}"#);
        }
    });
    let result = run_script(
        &mut fake,
        "packets",
        r#"
        import { packet } from "aqx";
        const zones: string[] = [];
        packet.on("event", (p) => zones.push(p.data?.args.zoneSet));
        const reply = packet.request("%xt%zm%hi%1%", "hi");
        if (reply.cmd !== "hi" || reply.args.join() !== "") throw new Error(JSON.stringify(reply));
        if (zones.join() !== "A") throw new Error(`zones: ${zones.join()}`);
        "#,
    );
    finished(result);
    assert!(sent(fake).iter().any(|m| m == "%xt%zm%hi%1%"));
}

#[test]
fn cell_aura_and_counter_events_reach_handlers() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg == "%xt%zm%moveToCell%5%r2%Left%" {
            wire.object(
                r#"{"cmd":"ct","anims":[{"cInf":"m:2","tInf":"p:1001","msg":"Treeant prepares a counter attack!!"}],
                "a":[{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Rage","dur":5,"stk":2}]},{"cmd":"aura++","tInf":"m:2","auras":[{"nam":"Counter Attack","dur":6}]}]}"#,
            );
            wire.object(r#"{"cmd":"ct","a":[{"cmd":"aura--","tInf":"m:2","aura":{"nam":"Counter Attack"}}]}"#);
        }
    });
    let result = run_script(
        &mut fake,
        "service-events",
        r#"
        import { events, map } from "aqx";
        const seen: string[] = [];
        events.on("cell", (c) => seen.push(`cell ${c.cell}/${c.pad}`));
        events.on("aura", (a) => seen.push(`aura ${a.change} ${a.name} x${a.stacks} on ${a.monster ?? "me"}`));
        events.on("counter", (c) => seen.push(`counter ${c.monster} ${c.active}`));
        map.jump("r2");
        const want = ["cell r2/Left", "aura added Rage x2 on me", "aura added Counter Attack x1 on 2", "counter 2 true", "aura removed Counter Attack x0 on 2", "counter 2 false"];
        if (seen.join("; ") !== want.join("; ")) throw new Error(seen.join("; "));
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn aggro_takes_a_predicate_and_ends_with_the_script() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = run_script(
        &mut fake,
        "aggro",
        r#"
        import { combat, map, wait } from "aqx";
        map.jump("r2");
        combat.aggro((m) => m.monId === 12);
        wait.ms(700);
        "#,
    );
    finished(result);
    let _ = fake.session.pump_until(Some(Instant::now() + Duration::from_millis(800)), |_| None::<()>);
    let pulls = sent(fake).into_iter().filter(|m| m.contains("%aggroMon%")).collect::<Vec<_>>();
    assert!((1..=2).contains(&pulls.len()) && pulls.iter().all(|m| m == "%xt%zm%aggroMon%5%2%"), "{pulls:?}");
}

#[test]
fn the_player_knows_its_target_and_server() {
    let mut fake = Fake::start(FARM, killed_on(2));
    let result = run_script(
        &mut fake,
        "target",
        r#"
        import { combat, player, session } from "aqx";
        let seen = "";
        combat.kill("Treeant", { hold: () => { seen ||= player.target?.name ?? ""; return false; } });
        if (seen !== "Treeant" || player.target !== null) throw new Error(`${seen} ${JSON.stringify(player.target)}`);
        if (session.server !== "Fake" || session.servers()[0]?.name !== "Fake") throw new Error(session.server);
        "#,
    );
    finished(result);
    fake.finish();
}

#[test]
fn a_script_can_relogin_and_carry_on() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = run_script(
        &mut fake,
        "relogin",
        r#"
        import { map, session } from "aqx";
        map.jump("r1");
        session.relogin();
        if (map.name !== "farm") throw new Error(map.name);
        map.jump("r2");
        "#,
    );
    finished(result);
    assert_eq!(fake.logins.load(std::sync::atomic::Ordering::SeqCst), 2);
    fake.finish();
}

#[track_caller]
fn check_fails(test: &str, files: &[(&str, &str)], text: &str) {
    let result = check(&write(test, files));
    assert!(matches!(&result, Err(why) if why.contains(text)), "wanted an error mentioning {text:?}, got {result:?}");
}

#[test]
fn check_loads_every_import_without_running_the_script() {
    let main = r#"import { player } from "aqx"; import { double } from "./lib.ts"; throw new Error("ran " + double(player.hp));"#;
    assert_eq!(check(&write("check-ok", &[("main.ts", main), ("lib.ts", "export const double = (n: number): number => n * 2;")])), Ok(()));
    check_fails("check-syntax", &[("main.ts", "let = ;")], "Expression expected");
    check_fails("check-npm", &[("main.ts", r#"import fs from "fs";"#)], "only `aqx` and relative imports are supported");
    check_fails("check-missing", &[("main.ts", r#"import { x } from "./gone.ts";"#)], "check-missing/gone.ts': No such file");
    check_fails("check-aqx-export", &[("main.ts", r#"import { combt } from "aqx";"#)], "Could not find export 'combt' in module 'aqx'");
    check_fails("check-export", &[("main.ts", r#"import { y } from "./lib.ts";"#), ("lib.ts", "export const x = 1;")], "Could not find export 'y'");
    check_fails("check-enum", &[("main.ts", r#"import { E } from "./lib.ts";"#), ("lib.ts", "export enum E { A }")], "export enum E { A }\n  |        ^^^^^^^^^^^^");
}

#[test]
fn the_player_reads_its_entity_id_and_combat_stats() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%") {
            wire.object(r#"{"cmd":"balance","intGold":10,"intCoins":50}"#);
            wire.object(r#"{"cmd":"stu","sta":{"$STR":7,"$ap":14,"$sp":37,"$tcr":0.08,"$scm":1.66,"$tha":0.03}}"#);
        }
    });
    let result = run_script(
        &mut fake,
        "stats",
        r#"
        import { map, player, wait } from "aqx";
        map.jump("r2", "Left");
        if (!wait.until(() => player.stats.critMultiplier > 0, 2000)) throw new Error("no stats arrived");
        if (player.id !== 1001) throw new Error(`id: ${player.id}`);
        if (player.coins !== 50) throw new Error(`coins: ${player.coins}`);
        const { str, attackPower, spellPower, crit, critMultiplier } = player.stats;
        const seen = [str, attackPower, spellPower, crit, critMultiplier].join();
        if (seen !== "7,14,37,0.08,1.66") throw new Error(`stats: ${seen}`);
        "#,
    );
    finished(result);
}

#[test]
fn a_monster_shows_when_it_is_in_combat() {
    let mut fake = Fake::start(FARM, |msg, wire| {
        if msg.contains("%moveToCell%") {
            wire.object(r#"{"cmd":"ct","m":{"2":{"intHP":800,"intState":2}}}"#);
        }
    });
    let result = run_script(
        &mut fake,
        "monster-state",
        r#"
        import { map, wait } from "aqx";
        map.jump("r2", "Left");
        const treeant = () => map.find("treeant")[0];
        if (!wait.until(() => treeant().state === "combat", 2000)) throw new Error(`state: ${treeant().state}`);
        "#,
    );
    finished(result);
}

#[test]
fn the_inventory_reports_its_misc_pool_apart_from_the_bag() {
    let mut fake = Fake::start(FARM, |_, _| {});
    let result = run_script(
        &mut fake,
        "misc-pool",
        r#"
        import { inventory } from "aqx";
        if (inventory.misc.slots !== 100 || inventory.misc.used !== 0) throw new Error(`misc ${inventory.misc.slots} ${inventory.misc.used}`);
        "#,
    );
    finished(result);
}
