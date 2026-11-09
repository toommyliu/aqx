use std::time::Duration;

use aqx_client::ItemId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::style::Color;

use std::sync::mpsc::{self, Receiver};

use super::*;
use crate::control::ServerChoice;
use crate::fleet::{Fleet, Stats};
use crate::view::{Game, Player, QuestProgress, Shown, Stack, Standing, State};

fn stack(id: i64, name: &str, qty: u32, max: u32) -> Stack {
    Stack {
        id: ItemId(id),
        name: name.into(),
        qty,
        max,
        equipped: false,
        rank: None,
    }
}

fn farmer(name: &str) -> Shown {
    let mut inventory = vec![
        Stack {
            equipped: true,
            rank: Some(10),
            ..stack(1, "Master Ranger", 302_500, 1)
        },
        stack(2, "Fishing Bait", 533, 999),
        stack(3, "Paint", 99, 99),
    ];
    inventory.extend((0..35).map(|i| stack(100 + i, &format!("Spare Part {i:02}"), 1, 1)));
    Shown {
        name: name.into(),
        user: name.into(),
        state: State::Running,
        server: "Twilly".into(),
        script: Some("/srv/farm/void-aura.ts".into()),
        status: "Astral Ephemerite Essence".into(),
        up: Some(Duration::from_secs(90)),
        stats: Stats {
            kills: 12,
            ..Stats::default()
        },
        gold_gained: Some(1500),
        game: Some(Game {
            area: "farm-1".into(),
            cell: "Enter".into(),
            pad: "Spawn".into(),
            hp: 2000,
            max_hp: 4000,
            mp: 100,
            max_mp: 100,
            class: "Barber".into(),
            class_rank: 10,
            level: 100,
            gold: 12_300_000,
            players: vec![Player {
                name: "player2".into(),
                level: Some(40),
            }],
            drops: (0..8)
                .map(|i| stack(200 + i, &format!("Spider Drop {i}"), i as u32 + 1, 99))
                .collect(),
            quests: vec![
                QuestProgress {
                    id: 1,
                    name: "Spider Hunt".into(),
                    have: 4,
                    need: 10,
                },
                QuestProgress {
                    id: 2,
                    name: "Free Gift".into(),
                    have: 0,
                    need: 0,
                },
            ],
            factions: vec![
                Standing {
                    name: "Brightoak".into(),
                    rank: 4,
                    rep: 1200,
                    next: Some(12_500),
                    gained: true,
                },
                Standing {
                    name: "Aegis".into(),
                    rank: 1,
                    rep: 0,
                    next: Some(900),
                    gained: false,
                },
            ],
            temp: vec![stack(300, "Spider Silk", 4, 10)],
            inventory,
            bag_used: 38,
            bag_slots: 40,
            bank_used: 120,
            bank_slots: 200,
            ..Game::default()
        }),
    }
}

fn offline(name: &str) -> Shown {
    Shown {
        state: State::Offline { why: None },
        script: None,
        status: String::new(),
        up: None,
        gold_gained: None,
        game: None,
        server: String::new(),
        ..farmer(name)
    }
}

fn idle_fleet() -> Fleet {
    Fleet::of(Vec::new())
}

fn push(app: &mut App, channel: Channel, text: &str) {
    app.log.push(channel, text.into());
}

fn cells(app: &App, shown: &[Shown], width: u16, height: u16) -> Vec<Vec<ratatui::buffer::Cell>> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| draw::draw(f, app, shown)).unwrap();
    let content = &terminal.backend().buffer().content;
    content.chunks(width as usize).map(<[_]>::to_vec).collect()
}

fn screen(app: &App, shown: &[Shown], width: u16, height: u16) -> String {
    cells(app, shown, width, height)
        .iter()
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn row<'a>(screen: &'a str, has: &str) -> &'a str {
    screen
        .lines()
        .find(|l| l.contains(has))
        .unwrap_or_else(|| panic!("no row has {has:?}:\n{screen}"))
}

fn accented(app: &App, shown: &[Shown], width: u16, height: u16, has: &str) -> String {
    let rows = cells(app, shown, width, height);
    let line = rows
        .iter()
        .find(|r| {
            r.iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .contains(has)
        })
        .unwrap_or_else(|| panic!("no row has {has:?}"));
    line.iter()
        .filter(|c| c.fg == Color::Blue)
        .map(|c| c.symbol())
        .collect()
}

fn lit(app: &App, shown: &[Shown], width: u16, height: u16, has: &str) -> Vec<String> {
    let rows = cells(app, shown, width, height);
    let line = rows
        .iter()
        .find(|r| {
            r.iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .contains(has)
        })
        .unwrap_or_else(|| panic!("no row has {has:?}"));
    let mut runs: Vec<String> = Vec::new();
    let mut last = false;
    for c in line {
        let on = c.bg == Color::Yellow;
        match runs.last_mut() {
            Some(run) if on && last => run.push_str(c.symbol()),
            _ if on => runs.push(c.symbol().to_owned()),
            _ => {}
        }
        last = on;
    }
    runs
}

fn press(app: &mut App, shown: &[Shown], code: KeyCode) {
    app.handle(KeyEvent::from(code), &idle_fleet(), shown);
}

fn click(app: &mut App, shown: &[Shown], width: u16, height: u16, has: &str) {
    let rows = cells(app, shown, width, height);
    let (y, line) = rows
        .iter()
        .enumerate()
        .map(|(y, r)| (y, r.iter().map(|c| c.symbol()).collect::<Vec<_>>()))
        .find(|(_, r)| r.concat().contains(has))
        .unwrap_or_else(|| panic!("no row has {has:?}"));
    let text = line.concat();
    let column = text[..text.find(has).unwrap()].chars().count();
    let at = MouseEvent {
        kind: MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        column: column as u16,
        row: y as u16,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(at, shown);
}

#[test]
fn a_number_opens_its_view_directly_and_esc_returns_to_the_log() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    push(&mut app, Channel::Server, "Profanity filter On.");

    for (key, title) in [
        ('4', "── 4 quests 2 · in the order accepted"),
        ('2', "── 2 players 1"),
        (
            '6',
            "── 6 inventory 38/40 · equipped first, then last changed",
        ),
        ('8', "── 8 factions 2 · gained first, then by name"),
    ] {
        press(&mut app, &shown, KeyCode::Char(key));
        let s = screen(&app, &shown, 150, 30);
        assert!(s.contains(title), "{key} opens {title:?}:\n{s}");
        assert!(
            !s.contains("Profanity filter"),
            "the list takes the log's place:\n{s}"
        );
    }
    let factions = screen(&app, &shown, 150, 30);
    assert!(
        row(&factions, "Aegis").contains("rank 1 0/900"),
        "{factions}"
    );

    press(&mut app, &shown, KeyCode::Char('8'));
    assert_eq!(
        app.view,
        View::List(Section::Factions),
        "the open view's number keeps it open"
    );
    press(&mut app, &shown, KeyCode::Esc);
    assert_eq!(app.view, View::Log);
    press(&mut app, &shown, KeyCode::Char('7'));
    press(&mut app, &shown, KeyCode::Char('1'));
    assert_eq!(app.view, View::Log, "1 is the log");
    assert!(screen(&app, &shown, 150, 30).contains("Profanity filter"));
}

#[test]
fn brackets_step_through_visible_log_channels_without_wrapping() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    push(&mut app, Channel::Script, "farming essence");
    push(&mut app, Channel::Chat, "[zone] player2: hi");
    push(&mut app, Channel::Game, "killed Mosquito, +10 gold, +5 xp");

    let main = screen(&app, &shown, 150, 30);
    assert!(
        row(
            &main,
            "── log  main · all · aqx · script · server · chat · game"
        )
        .contains('┬'),
        "{main}"
    );
    assert_eq!(accented(&app, &shown, 150, 30, "── log"), "main");
    assert!(
        row(&main, "farming essence").contains("script  farming essence"),
        "main tags each line:\n{main}"
    );
    assert!(
        !main.contains("player2: hi") && !main.contains("killed Mosquito"),
        "main leaves out chat and game:\n{main}"
    );

    press(&mut app, &shown, KeyCode::Char('['));
    assert_eq!(app.filter, Filter::Main, "[ stops at the first channel");
    press(&mut app, &shown, KeyCode::Char(']'));
    let all = screen(&app, &shown, 150, 30);
    assert_eq!(accented(&app, &shown, 150, 30, "── log"), "all");
    assert!(
        all.contains("player2: hi") && all.contains("killed Mosquito"),
        "{all}"
    );

    for _ in 0..10 {
        press(&mut app, &shown, KeyCode::Char(']'));
    }
    assert_eq!(
        app.filter,
        Filter::Only(Channel::Game),
        "] stops at the last channel"
    );
    let game = screen(&app, &shown, 150, 30);
    assert!(
        !row(&game, "killed Mosquito").contains("game  killed"),
        "one channel needs no tag:\n{game}"
    );
    assert!(!game.contains("farming essence"), "{game}");

    press(&mut app, &shown, KeyCode::Char('3'));
    press(&mut app, &shown, KeyCode::Char('['));
    assert_eq!(
        (app.view, app.filter),
        (View::Log, Filter::Only(Channel::Chat)),
        "a bracket from a list goes back to the log on the next channel"
    );
}

#[test]
fn the_rail_labels_each_list_with_its_key_and_a_click_opens_it() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    let s = screen(&app, &shown, 150, 34);
    for heading in [
        "│ 2 Players 1",
        "│ 3 Drops 8",
        "│ 4 Quests 2",
        "│ 5 Temp 1",
        "│ 6 Inventory 38/40",
        "│ 7 Bank 120/200  not opened",
        "│ 8 Factions 2",
    ] {
        assert!(s.contains(heading), "missing {heading:?}:\n{s}");
    }
    assert!(
        row(&s, "Spider Hunt").contains("4/10") && row(&s, "Free Gift").contains("ready"),
        "{s}"
    );
    assert!(
        !s.contains("Aegis"),
        "the rail lists only factions that gained rep:\n{s}"
    );

    click(&mut app, &shown, 150, 34, "4 Quests");
    assert_eq!(app.view, View::List(Section::Quests));
    let quests = screen(&app, &shown, 150, 34);
    assert!(
        quests.contains("│▌4 Quests 2"),
        "the open list is marked in the rail:\n{quests}"
    );
    click(&mut app, &shown, 150, 34, "4 Quests");
    assert_eq!(
        app.view,
        View::Log,
        "clicking the open list again goes back to the log"
    );

    click(&mut app, &shown, 150, 34, "server ·");
    assert_eq!(
        app.filter,
        Filter::Only(Channel::Server),
        "a channel on the divider is clickable"
    );
}

#[test]
fn without_the_rail_the_divider_lists_every_view_as_a_tab() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('z'));
    let s = screen(&app, &shown, 150, 30);
    let divider = row(&s, "── 1 log·main");
    assert!(
        divider.contains("2 players 1  3 drops 8  4 quests 2  5 temp 1  6 inventory 38/40  7 bank 120/200  8 factions 2"),
        "{s}"
    );
    assert!(!divider.contains('┬'), "{s}");
    assert_eq!(accented(&app, &shown, 150, 30, "── 1 log"), "1 log·main");

    press(&mut app, &shown, KeyCode::Char('3'));
    assert_eq!(accented(&app, &shown, 150, 30, "── 1 log"), "3 drops 8");
    let narrow = screen(&app, &shown, 80, 30);
    assert!(
        row(&narrow, "── 1 log·main").contains("3 drops  4 quests"),
        "a narrow pane drops the counts first:\n{narrow}"
    );

    click(&mut app, &shown, 150, 30, "7 bank");
    assert_eq!(app.view, View::List(Section::Bank));
    assert!(screen(&app, &shown, 150, 30).contains("The bank shows here once a script opens it."));
}

#[test]
fn an_offline_account_keeps_the_log_and_lists_stay_shut() {
    let shown = [offline("player1")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('3'));
    assert_eq!(app.view, View::Log);
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("── log  main · all"), "{s}");
    assert!(row(&s, "player1").contains("○ offline"), "{s}");
    assert!(s.contains("i log in  o log in and run"), "{s}");
}

#[test]
fn the_drops_list_marks_drops_for_one_pick_up() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('3'));
    for key in [
        KeyCode::Char(' '),
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Char(' '),
    ] {
        let _ = screen(&app, &shown, 150, 30);
        press(&mut app, &shown, key);
    }
    assert_eq!(app.picked, HashSet::from([ItemId(200), ItemId(202)]));
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("· 2 marked"), "{s}");
    assert!(
        s.contains("  ◆Spider Drop 0")
            && s.contains("   Spider Drop 1")
            && s.contains(" ▌◆Spider Drop 2"),
        "{s}"
    );
    assert!(s.contains("enter accept marked"), "{s}");
    press(&mut app, &shown, KeyCode::Char('a'));
    assert_eq!(app.picked.len(), 8);
    press(&mut app, &shown, KeyCode::Esc);
    assert!(
        app.picked.is_empty() && app.view == View::List(Section::Drops),
        "esc clears the marks first"
    );
    press(&mut app, &shown, KeyCode::Esc);
    assert_eq!(app.view, View::Log);
}

#[test]
fn the_sidebar_lists_each_account_with_offline_ones_last() {
    let shown = [offline("player1"), farmer("player2"), farmer("player3")];
    let mut app = App::default();
    let s = screen(&app, &shown, 150, 30);
    let lines: Vec<&str> = s.lines().skip(2).take(4).collect();
    assert!(
        lines[0].contains("player2") && lines[0].contains("void-aura.ts"),
        "{s}"
    );
    assert!(lines[1].contains("player3"), "{s}");
    assert!(lines[2].contains("── offline 1"), "{s}");
    assert!(
        lines[3].contains("▌") && lines[3].contains("player1") && lines[3].contains("logged out"),
        "{s}"
    );

    press(&mut app, &shown, KeyCode::Up);
    assert_eq!(app.selected, 2, "up walks the sidebar in its order");
    press(&mut app, &shown, KeyCode::Tab);
    press(&mut app, &shown, KeyCode::Up);
    assert_eq!(app.selected, 2, "with the main pane focused, up scrolls it");
    click(&mut app, &shown, 150, 30, "player2");
    assert_eq!(app.selected, 1);
    assert!(!app.pane_focus, "a click in the sidebar focuses it");
}

#[test]
fn the_log_holds_still_while_scrolled_up_and_counts_what_arrives() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    for i in 0..60 {
        push(&mut app, Channel::Script, &format!("line {i:02}"));
    }
    let _ = screen(&app, &shown, 150, 30);
    press(&mut app, &shown, KeyCode::PageUp);
    let before = screen(&app, &shown, 150, 30);
    assert!(before.contains("jump to bottom (End)"), "{before}");
    app.receive(Line {
        seq: app.log.next_seq(),
        at_ms: 0,
        channel: Channel::Script,
        text: "line 60".into(),
    });
    let after = screen(&app, &shown, 150, 30);
    let visible = |s: &str| {
        s.lines()
            .filter(|l| l.contains("line "))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(visible(&before), visible(&after), "{after}");
    assert!(after.contains("1 new · jump to bottom (End)"), "{after}");
    press(&mut app, &shown, KeyCode::End);
    assert!(screen(&app, &shown, 150, 30).contains("line 60"));
}

#[test]
fn q_detaches_and_shift_q_or_ctrl_c_asks_to_end_the_session() {
    let key = |c| KeyEvent::from(KeyCode::Char(c));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let shown = [farmer("player1"), offline("player2")];
    let mut app = App::default();
    assert_eq!(
        app.handle(key('q'), &idle_fleet(), &shown),
        Some(Leave::Detach)
    );
    assert!(
        screen(&app, &shown, 150, 30).contains("q detach"),
        "the header offers detaching"
    );

    assert_eq!(app.handle(key('Q'), &idle_fleet(), &shown), None);
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("End the aqx session?"), "{s}");
    assert!(
        s.contains("This logs out the account that is online.")
            && s.contains("Detaching leaves them running."),
        "{s}"
    );
    press(&mut app, &shown, KeyCode::Esc);
    assert!(!app.ending);

    assert_eq!(
        app.handle(ctrl_c, &idle_fleet(), &shown),
        None,
        "ctrl+c asks first"
    );
    assert!(app.ending);
    assert_eq!(
        app.handle(ctrl_c, &idle_fleet(), &shown),
        Some(Leave::Detach),
        "a second ctrl+c detaches"
    );

    app.ending = true;
    assert_eq!(
        app.handle(key('q'), &idle_fleet(), &shown),
        Some(Leave::Detach)
    );
    app.ending = true;
    assert_eq!(
        app.handle(KeyEvent::from(KeyCode::Enter), &idle_fleet(), &shown),
        None,
        "ending keeps the TUI up while the accounts log out"
    );
    assert!(app.closing && !app.ending);
    let s = screen(&app, &shown, 150, 30);
    let footer = s.lines().last().unwrap();
    assert!(
        footer.contains("logging out 1 account; the session ends once it's offline"),
        "{footer}"
    );
}

#[test]
fn d_shows_every_detail() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    let short = screen(&app, &shown, 150, 30);
    assert!(
        row(&short, "HP").contains("12 kills") && row(&short, "HP").contains("+1500 gold"),
        "{short}"
    );
    press(&mut app, &shown, KeyCode::Char('d'));
    let full = screen(&app, &shown, 150, 30);
    assert!(
        row(&full, "MP").contains("Barber rank 10") && row(&full, "MP").contains("12.3M gold"),
        "{full}"
    );
    assert!(
        full.contains("no target") && full.contains("0 deaths"),
        "{full}"
    );
}

#[test]
fn the_last_lines_of_each_account_print_after_quitting() {
    let accounts = ["player1", "player2"]
        .map(|name| std::sync::Arc::new(crate::fleet::Account::detached(name)));
    for (channel, text) in [
        (Channel::Aqx, "logging in to Twilly"),
        (Channel::Script, "farming"),
        (Channel::Aqx, "the script stopped: done"),
        (Channel::Game, "killed Frogzard, +10 gold, +5 xp"),
        (Channel::Aqx, "disconnected: logged out"),
    ] {
        accounts[0].board().log.push(channel, text.into());
    }
    accounts[1]
        .board()
        .log
        .push(Channel::Aqx, "couldn't log in: player2 is banned".into());
    let fleet = Fleet::of(accounts.to_vec());
    let words = last_words(&fleet.last_lines(), false);
    let expected = [
        "farming",
        "the script stopped: done",
        "disconnected: logged out",
        "couldn't log in: player2 is banned",
    ];
    assert_eq!(
        words.len(),
        expected.len(),
        "the last 3 lines of each, without game events: {words:?}"
    );
    assert!(
        words
            .iter()
            .zip(expected)
            .all(|(w, e)| w.ends_with(&format!("  {e}"))),
        "{words:?}"
    );
    assert!(words[0].contains("  player1  script  farming"), "{words:?}");

    let redacted = last_words(&fleet.last_lines(), true);
    assert!(
        redacted[3].contains("account 2  aqx     couldn't log in: account 2 is banned"),
        "{redacted:?}"
    );

    let one = Fleet::of(accounts[1..].to_vec());
    assert!(
        !last_words(&one.last_lines(), false)[0].contains("player2  aqx"),
        "one account needs no name column"
    );
}

#[test]
fn redact_shows_accounts_by_number() {
    let fleet = Fleet::of(vec![std::sync::Arc::new(crate::fleet::Account::detached(
        "player1",
    ))]);
    let mut app = App::new(&[], Path::new("/srv"), History::default(), true);
    let shown = gather(&fleet, &mut app).unwrap().unwrap();
    app.receive(Line {
        seq: 9,
        at_ms: 0,
        channel: Channel::Chat,
        text: "[whisper] player2: hi player1".into(),
    });
    app.filter = Filter::All;
    let s = screen(&app, &shown, 150, 30);
    assert!(!s.contains("player1"), "{s}");
    assert!(row(&s, "○ offline").contains("account 1"), "{s}");
    let redacted = app.masked(farmer("player1"));
    assert_eq!(redacted.name, "account 1");
    assert!(
        s.contains("[whisper] player2: hi account 1"),
        "only this fleet's names are hidden:\n{s}"
    );
}

#[test]
fn a_paused_account_says_so_and_p_offers_to_resume() {
    let paused = Shown {
        state: State::Paused,
        ..farmer("player1")
    };
    let shown = [paused, farmer("player2")];
    let app = App::default();
    let s = screen(&app, &shown, 150, 30);
    assert!(row(&s, "▌ ‖ player1").contains("void-aura.ts,"), "{s}");
    assert_eq!(
        s.matches("Astral Ephemerite Essence").count(),
        1,
        "the status shows once:\n{s}"
    );
    assert!(row(&s, "player1 · Twilly").contains("‖ paused"), "{s}");
    assert!(s.contains("p resume"), "{s}");
}

#[test]
fn a_required_arg_is_asked_for_each_marked_account() {
    let home = std::env::temp_dir().join(format!("aqx-ask-tests-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("follow.ts"),
        "import { script } from \"aqx\";\nexport default function* () { script.args<{ leader: string }>(); }\n",
    )
    .unwrap();
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App {
        home: home.clone(),
        names: vec!["player1".into(), "player2".into()],
        marked: BTreeSet::from([0, 1]),
        ..App::default()
    };
    app.submit("follow.ts", &idle_fleet());
    let footer = |app: &App| {
        screen(app, &shown, 150, 30)
            .lines()
            .last()
            .unwrap()
            .to_owned()
    };
    assert!(
        footer(&app).contains("follow.ts needs leader for player1:"),
        "{}",
        footer(&app)
    );
    assert!(
        footer(&app).contains("tab same for all"),
        "{}",
        footer(&app)
    );
    for c in "player3".chars() {
        press(&mut app, &shown, KeyCode::Char(c));
    }
    press(&mut app, &shown, KeyCode::Enter);
    assert!(
        footer(&app).contains("follow.ts needs leader for player2:"),
        "{}",
        footer(&app)
    );
    assert!(
        !footer(&app).contains("player3"),
        "the next account starts blank: {}",
        footer(&app)
    );
    for c in "player4".chars() {
        press(&mut app, &shown, KeyCode::Char(c));
    }
    press(&mut app, &shown, KeyCode::Enter);
    assert!(
        app.ask.is_none(),
        "both accounts answered, so the runs start"
    );

    app.submit("follow.ts", &idle_fleet());
    assert!(
        footer(&app).contains("for player1: player3"),
        "{}",
        footer(&app)
    );
    press(&mut app, &shown, KeyCode::Enter);
    assert!(
        footer(&app).contains("for player2: player4"),
        "each account gets its own last answer: {}",
        footer(&app)
    );

    std::fs::write(
        home.join("wait.ts"),
        "import { script } from \"aqx\";\nexport default function* () { script.args<{ wait?: number; class?: string }>({ wait: 5, class: \"\" }); }\n",
    )
    .unwrap();
    app.ask = None;
    app.marked.clear();
    app.submit("wait.ts", &idle_fleet());
    assert!(
        footer(&app).contains("wait.ts needs wait: █a number, enter uses 5"),
        "{}",
        footer(&app)
    );
    press(&mut app, &shown, KeyCode::Enter);
    assert!(
        footer(&app).contains("wait.ts needs class: █text, optional: enter skips"),
        "an empty default reads as skipped: {}",
        footer(&app)
    );
}

struct Frozen(Vec<Shown>);

impl Source for Frozen {
    fn frame(&self, _: &crate::control::Want) -> Result<crate::control::Frame, String> {
        Ok(crate::control::Frame {
            accounts: self.0.clone(),
            log: Vec::new(),
            ending: false,
        })
    }

    fn act(&self, _: usize, _: Action) -> Result<(), String> {
        Ok(())
    }

    fn servers(&self, _: &[usize]) -> Receiver<Result<Vec<ServerChoice>, String>> {
        mpsc::channel().1
    }

    fn configured(&self) -> Vec<crate::config::Script> {
        Vec::new()
    }

    fn end(&self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn up_in_the_run_prompt_offers_scripts_running_elsewhere_after_your_own() {
    let mut app = App {
        home: PathBuf::from("/srv"),
        ..App::default()
    };
    app.recent.add(crate::config::Script {
        path: PathBuf::from("/srv/mine.ts"),
        args: Vec::new(),
    });
    let source = Frozen(vec![
        farmer("player1"),
        farmer("player2"),
        offline("player3"),
    ]);
    gather(&source, &mut app).unwrap();
    let shown = gather(&source, &mut app).unwrap().unwrap();
    let typed = |app: &App| app.prompt.as_ref().unwrap().input.value().to_owned();
    press(&mut app, &shown, KeyCode::Char('O'));
    press(&mut app, &shown, KeyCode::Up);
    assert_eq!(typed(&app), "mine.ts");
    press(&mut app, &shown, KeyCode::Up);
    assert_eq!(
        typed(&app),
        "farm/void-aura.ts",
        "the accounts run it, though nobody typed it here"
    );
    press(&mut app, &shown, KeyCode::Up);
    press(&mut app, &shown, KeyCode::Down);
    assert_eq!(typed(&app), "mine.ts", "it's offered once");
}

#[test]
fn a_paste_lands_in_the_open_input_at_once_and_never_submits() {
    let home = std::env::temp_dir().join(format!("aqx-paste-tests-{}", std::process::id()));
    std::fs::create_dir_all(home.join("farm")).unwrap();
    std::fs::write(
        home.join("farm/follow.ts"),
        "import { script } from \"aqx\";\nexport default function* () { script.args<{ leader: string }>(); }\n",
    )
    .unwrap();
    let shown = [farmer("player1")];
    let mut app = App {
        home: home.clone(),
        names: vec!["player1".into()],
        ..App::default()
    };
    app.paste("ignored", &shown);
    assert!(
        app.prompt.is_none() && app.query.value().is_empty(),
        "with no input open, a paste does nothing"
    );

    press(&mut app, &shown, KeyCode::Char('O'));
    app.paste("farm/follow.ts\n", &shown);
    let prompt = app.prompt.as_ref().expect("the run prompt stays open");
    assert_eq!(
        prompt.input.value(),
        "farm/follow.ts",
        "the line break doesn't submit it"
    );
    assert_eq!(
        prompt.matches,
        ["farm/follow.ts"],
        "completion follows the pasted path"
    );

    press(&mut app, &shown, KeyCode::Enter);
    assert!(app.ask.is_some(), "the leader arg is asked for");
    app.ask.as_mut().unwrap().input = TextInput::new("offered".into());
    app.ask.as_mut().unwrap().recalled = true;
    app.paste("player2", &shown);
    assert_eq!(
        app.ask.as_ref().unwrap().input.value(),
        "player2",
        "a paste replaces an offered answer"
    );
}

fn type_in(app: &mut App, shown: &[Shown], text: &str) {
    for c in text.chars() {
        press(app, shown, KeyCode::Char(c));
    }
}

#[test]
fn slash_searches_the_open_log_and_esc_clears_it() {
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App::default();
    push(&mut app, Channel::Script, "Killed Void Spider");
    push(&mut app, Channel::Server, "Profanity filter On.");

    press(&mut app, &shown, KeyCode::Tab);
    press(&mut app, &shown, KeyCode::Char('/'));
    type_in(&mut app, &shown, "void");
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains(" search the log: void"), "{s}");
    assert!(
        s.contains("Killed Void Spider") && !s.contains("Profanity filter"),
        "{s}"
    );
    assert_eq!(lit(&app, &shown, 150, 30, "Killed Void Spider"), ["Void"]);
    assert!(
        s.contains("player2"),
        "the sidebar keeps every account:\n{s}"
    );

    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Tab);
    press(&mut app, &shown, KeyCode::Down);
    let s = screen(&app, &shown, 150, 30);
    assert_eq!(app.selected, 1);
    assert!(
        s.contains("search \"void\"") && !s.contains("Profanity filter"),
        "the search stays on when the account changes:\n{s}"
    );

    press(&mut app, &shown, KeyCode::Tab);
    type_in(&mut app, &shown, "/x");
    assert!(
        screen(&app, &shown, 150, 30).contains("No lines match \"voidx\"."),
        "/ edits the search"
    );
    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Esc);
    assert!(app.query.value().is_empty());
    assert!(screen(&app, &shown, 150, 30).contains("Profanity filter"));
}

#[test]
fn slash_searches_the_open_list_and_another_view_clears_it() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('6'));
    press(&mut app, &shown, KeyCode::Char('/'));
    type_in(&mut app, &shown, "BAIT");
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains(" search inventory: BAIT"), "{s}");
    assert!(
        s.contains("Fishing Bait") && !s.contains("Master Ranger"),
        "{s}"
    );
    assert_eq!(lit(&app, &shown, 150, 30, "Fishing Bait"), ["Bait"]);
    type_in(&mut app, &shown, "zz");
    assert!(screen(&app, &shown, 150, 30).contains("Nothing matches \"BAITzz\"."));

    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Char('3'));
    assert!(app.query.value().is_empty(), "a new view starts unsearched");

    press(&mut app, &shown, KeyCode::Char('/'));
    type_in(&mut app, &shown, "drop 5");
    press(&mut app, &shown, KeyCode::Enter);
    let s = screen(&app, &shown, 150, 30);
    assert!(
        s.contains(" ▌ Spider Drop 5") && !s.contains("Spider Drop 4"),
        "{s}"
    );
    assert!(s.contains("esc clear search"), "{s}");
    press(&mut app, &shown, KeyCode::Char(' '));
    assert_eq!(
        app.picked,
        HashSet::from([ItemId(205)]),
        "space marks the drop under the cursor"
    );
    press(&mut app, &shown, KeyCode::Esc);
    press(&mut app, &shown, KeyCode::Esc);
    assert!(
        app.query.value().is_empty() && app.view == View::List(Section::Drops),
        "esc clears the marks, then the search"
    );
    press(&mut app, &shown, KeyCode::Esc);
    assert_eq!(app.view, View::Log);
}

#[test]
fn slash_on_the_dashboard_filters_accounts() {
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('g'));
    press(&mut app, &shown, KeyCode::Char('/'));
    type_in(&mut app, &shown, "player2");
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains(" filter accounts: player2"), "{s}");
    assert!(!s.contains("player1"), "{s}");
    assert_eq!(app.selected, 1);

    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Enter);
    assert!(
        !app.grid && app.query.value().is_empty(),
        "opening an account drops the account filter"
    );
}

#[test]
fn a_match_split_by_wrapping_lights_up_on_both_rows() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    push(
        &mut app,
        Channel::Script,
        &format!("{} Moonstone Shard", "x".repeat(37)),
    );
    press(&mut app, &shown, KeyCode::Char('/'));
    type_in(&mut app, &shown, "moonstone sh");
    let s = screen(&app, &shown, 72, 20);
    assert!(row(&s, "Shard").trim() == "Shard", "{s}");
    assert_eq!(lit(&app, &shown, 72, 20, "Moonstone"), ["Moonstone"]);
    assert_eq!(lit(&app, &shown, 72, 20, "Shard"), ["Sh"]);
}

fn setting(
    group: &str,
    name: &str,
    kind: aqx_script::skua::Kind,
    value: Option<&str>,
) -> aqx_script::skua::Setting {
    aqx_script::skua::Setting {
        group: group.into(),
        name: name.into(),
        label: format!("{name} label"),
        description: format!("what {name} does"),
        kind,
        choices: match kind {
            aqx_script::skua::Kind::Choice => vec!["Base".into(), "Solo".into(), "Farm".into()],
            _ => Vec::new(),
        },
        value: value.map(Into::into),
    }
}

fn skua_options() -> skua::Skua {
    use aqx_script::skua::{Kind, Settings};
    let mut skua = skua::Skua::new(0, "Player1".into(), "/srv/farm/doall.cs".into());
    skua.loaded(
        skua::Tab::CoreBots,
        false,
        Ok(Settings {
            storage: None,
            options: vec![
                setting("Options", "PrivateRooms", Kind::Bool, None),
                setting("Options", "HuntDelayNr", Kind::Number, Some("100")),
                setting("Loadout", "SoloModeSelect", Kind::Choice, None),
            ],
        }),
    );
    skua.loaded(
        skua::Tab::Script,
        false,
        Ok(Settings {
            storage: Some("DoAll".into()),
            options: vec![
                setting("Options", "Fast", Kind::Bool, Some("True")),
                setting("Extra_Group", "Tries", Kind::Number, Some("3")),
            ],
        }),
    );
    skua
}

fn saved(tab: skua::Tab, key: &str, value: &str) -> skua::Outcome {
    skua::Outcome::Save(skua::Save {
        tab,
        changes: vec![(key.into(), value.into())],
    })
}

#[test]
fn right_arrow_in_the_picker_opens_a_skua_scripts_options() {
    let mut picker = skua_library();
    picker.key(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Right)),
        library::Outcome::Options("/srv/skua/Scripts/Farm/VoidAura.cs".into())
    );
    picker.key(KeyEvent::from(KeyCode::Char('/')));
    for c in "token".chars() {
        picker.key(KeyEvent::from(KeyCode::Char(c)));
    }
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Right)),
        library::Outcome::Options("/srv/skua/Scripts/Legion/LegionToken.cs".into()),
        "→ at the end of a search opens the options of what it found"
    );
    picker.tab = library::Tab::Files;
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Right)),
        library::Outcome::Nothing,
        "an empty folder has nothing to open"
    );

    let shown = [farmer("player1")];
    let mut app = App {
        picker: Some(skua_library()),
        ..App::default()
    };
    let s = screen(&app, &shown, 150, 30);
    assert!(s.lines().last().unwrap().contains("→ options"), "{s}");
    app.skua = Some(skua_options());
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("Skua options · player1"), "{s}");
    press(&mut app, &shown, KeyCode::Esc);
    assert!(
        app.skua.is_none() && app.picker.is_some(),
        "esc goes back to the picker"
    );
}

#[test]
fn the_picker_opens_on_the_accounts_script_wherever_it_is() {
    let dir = std::env::temp_dir().join(format!("aqx-picker-select-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["doall.cs", "farm.ts"] {
        std::fs::write(dir.join(file), "").unwrap();
    }
    let mut picker = skua_library();
    picker.tab = library::Tab::Files;
    picker.select(Path::new("/srv/skua/Scripts/Legion/LegionToken.cs"));
    assert_eq!(picker.tab, library::Tab::Skua);
    assert_eq!(picker.shown()[picker.cursor].name, "Legion Token");

    picker.select(&dir.join("farm.ts"));
    assert_eq!(
        (picker.tab, picker.folder()),
        (library::Tab::Files, dir.as_path())
    );
    assert_eq!(picker.shown()[picker.cursor].name, "farm.ts");
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Right)),
        library::Outcome::Nothing,
        "aqx scripts have no options"
    );
    picker.key(KeyEvent::from(KeyCode::Up));
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Right)),
        library::Outcome::Options(dir.join("doall.cs")),
        "a Skua script outside the library still has options"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skua_options_change_one_value_at_a_time() {
    use skua::{Outcome, Tab};
    let key = |skua: &mut skua::Skua, code| skua.key(KeyEvent::from(code), true);
    let mut skua = skua_options();
    assert_eq!(
        key(&mut skua, KeyCode::Enter),
        saved(Tab::CoreBots, "PrivateRooms", "True")
    );
    assert_eq!(
        key(&mut skua, KeyCode::Delete),
        Outcome::Nothing,
        "an unset key has nothing to clear"
    );
    key(&mut skua, KeyCode::Down);
    assert_eq!(
        key(&mut skua, KeyCode::Delete),
        saved(Tab::CoreBots, "HuntDelayNr", "")
    );
    assert_eq!(key(&mut skua, KeyCode::Enter), Outcome::Nothing);
    assert_eq!(
        skua.key(KeyEvent::from(KeyCode::Enter), false),
        Outcome::Nothing,
        "a script that started while typing keeps the edit unsaved"
    );
    assert!(skua.editing.is_some() && skua.error.is_some());
    for c in "\u{8}\u{8}\u{8}250".chars() {
        let code = if c == '\u{8}' {
            KeyCode::Backspace
        } else {
            KeyCode::Char(c)
        };
        key(&mut skua, code);
    }
    assert_eq!(
        key(&mut skua, KeyCode::Enter),
        saved(Tab::CoreBots, "HuntDelayNr", "250")
    );
    key(&mut skua, KeyCode::Down);
    assert_eq!(
        key(&mut skua, KeyCode::Left),
        saved(Tab::CoreBots, "SoloModeSelect", "Farm")
    );
    assert_eq!(
        key(&mut skua, KeyCode::Right),
        saved(Tab::CoreBots, "SoloModeSelect", "Base")
    );
    key(&mut skua, KeyCode::Tab);
    assert_eq!(
        key(&mut skua, KeyCode::Enter),
        saved(Tab::Script, "Fast", "False")
    );
    key(&mut skua, KeyCode::Down);
    assert_eq!(
        key(&mut skua, KeyCode::Delete),
        Outcome::Nothing,
        "script options have no unset state"
    );
    key(&mut skua, KeyCode::Enter);
    key(&mut skua, KeyCode::Char('0'));
    assert_eq!(
        key(&mut skua, KeyCode::Enter),
        saved(Tab::Script, "Extra_Group:Tries", "30")
    );
    assert_eq!(key(&mut skua, KeyCode::Esc), Outcome::Close);
}

#[test]
fn skua_options_are_read_only_while_the_script_runs() {
    let mut app = App::default();
    let shown = [Shown {
        script: Some("/srv/farm/doall.cs".into()),
        ..farmer("player1")
    }];
    app.skua = Some(skua_options());
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("CBO_Storage(player1).txt"), "{s}");
    assert!(row(&s, "PrivateRooms label").contains("default"), "{s}");
    assert!(s.contains("while the account is offline or idle"), "{s}");
    press(&mut app, &shown, KeyCode::Enter);
    let skua = app.skua.as_ref().unwrap();
    assert_eq!(skua.saving, None);
    assert!(skua.error.is_some());
    press(&mut app, &shown, KeyCode::Tab);
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("DoAll.cfg, shared by every account"), "{s}");
    assert!(s.contains("Extra Group"), "{s}");
    press(&mut app, &shown, KeyCode::Esc);
    assert!(app.skua.is_none());
    let relogging = [Shown {
        state: State::Reconnecting("in 5 s: kicked".into()),
        ..shown[0].clone()
    }];
    app.skua = Some(skua_options());
    press(&mut app, &relogging, KeyCode::Enter);
    let skua = app.skua.as_ref().unwrap();
    assert!(
        skua.saving.is_none() && skua.error.is_some(),
        "a relogging script keeps its options"
    );
}

#[test]
fn redacted_skua_options_show_the_accounts_alias() {
    let mut app = App {
        mask: mask(&[("player_one".into(), "Player_One".into())], true),
        ..App::default()
    };
    let shown = [offline("account 1")];
    let mut skua = skua_options();
    skua.user = "Player_One".into();
    if let skua::Pane::Ready(settings) = &mut skua.panes[0] {
        settings.options[0].value = Some("follow Player_One".into());
    }
    app.skua = Some(skua);
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("CBO_Storage(account 1).txt"), "{s}");
    assert!(
        !s.to_lowercase().contains("player_one") && !s.to_lowercase().contains("player one"),
        "{s}"
    );
}

struct Acts(RefCell<Vec<(usize, Action)>>);

impl Source for Acts {
    fn frame(&self, _: &crate::control::Want) -> Result<crate::control::Frame, String> {
        Err("no frames".into())
    }

    fn act(&self, account: usize, action: Action) -> Result<(), String> {
        self.0.borrow_mut().push((account, action));
        Ok(())
    }

    fn servers(&self, _: &[usize]) -> Receiver<Result<Vec<ServerChoice>, String>> {
        let choice = |name: &str, players, refusal: Option<&str>, members| ServerChoice {
            name: name.into(),
            players,
            capacity: 1000,
            members,
            refusal: refusal.map(str::to_owned),
        };
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(vec![
            choice("Twilly", 900, None, false),
            choice("Gravelyn", 1000, Some("is full"), false),
            choice("Sir Ver", 10, None, true),
            choice("Yorumi", 0, Some("is offline"), false),
        ]))
        .unwrap();
        rx
    }

    fn configured(&self) -> Vec<crate::config::Script> {
        Vec::new()
    }

    fn end(&self) -> Result<(), String> {
        Ok(())
    }
}

fn skua_library() -> Picker {
    use aqx_script::skua::library::{Entry, Library};
    let entry = |name: &str, path: &str, description: &str| Entry {
        name: name.into(),
        description: description.into(),
        tags: [
            "farm",
            "nulgath",
            "nation",
            "ultra",
            "shadow",
            "realm",
            "essence",
            "commanding",
            "army",
            "legion",
            "merge",
            "seasonal",
            "frostvale",
        ]
        .map(String::from)
        .to_vec(),
        path: path.into(),
    };
    Picker::new(
        library::Tab::Skua,
        PathBuf::from("/srv/skua/Scripts"),
        Ok(Some(Library {
            commit: "6c58f70aa".into(),
            entries: vec![
                entry("CoreBots", "CoreBots.cs", ""),
                entry(
                    "Void Aura",
                    "Farm/VoidAura.cs",
                    "Farms Void Auras in the Shadow Realm, then turns in the quest for Commanding Shadow Essences until the bank is full",
                ),
                entry(
                    "Legion Token",
                    "Legion/LegionToken.cs",
                    "Farms Legion Tokens",
                ),
            ],
        })),
    )
}

#[test]
fn a_searched_skua_script_runs_on_the_marked_accounts() {
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App {
        names: vec!["player1".into(), "player2".into()],
        marked: BTreeSet::from([0, 1]),
        picker: Some(skua_library()),
        ..App::default()
    };
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains("3 scripts · 6c58f70 · /srv/skua/Scripts"), "{s}");
    assert!(row(&s, "Void Aura").contains("Farm"), "{s}");
    assert!(s.lines().last().unwrap().contains("u update"), "{s}");

    app.marked.clear();
    click(&mut app, &shown, 150, 30, "player2");
    assert_eq!(app.selected, 0, "a click under the picker reaches nothing");
    app.marked = BTreeSet::from([0, 1]);

    let source = Acts(RefCell::new(Vec::new()));
    app.handle(KeyEvent::from(KeyCode::Char('/')), &source, &shown);
    app.paste("aura", &shown);
    let s = screen(&app, &shown, 150, 30);
    assert!(!s.contains("Legion Token"), "{s}");
    assert!(s.contains("Farms Void Auras"), "{s}");
    assert!(s.contains("until the bank is full"), "{s}");
    assert!(s.contains("#farm #nulgath"), "{s}");
    assert!(s.contains("#frostvale"), "tags wrap:\n{s}");
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);

    let run = Action::Run(crate::config::Script {
        path: "/srv/skua/Scripts/Farm/VoidAura.cs".into(),
        args: Vec::new(),
    });
    assert_eq!(*source.0.borrow(), [(0, run.clone()), (1, run)]);
    assert!(app.picker.is_none());
}

#[test]
fn the_files_tab_browses_folders_and_runs_a_script_with_its_args() {
    let dir = std::env::temp_dir().join(format!("aqx-picker-tests-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let scripts = dir.join("scripts");
    let farm = "import { script, type Script } from \"aqx\";\nexport default function* (): Script {\n\tconst { monster } = script.args<{ monster: string }>();\n}\n";
    for (file, text) in [
        ("farm/frog-hunt.ts", farm),
        ("quests.js", ""),
        ("types.d.ts", ""),
        ("notes.txt", ""),
        ("node_modules/lib/index.ts", ""),
        (".drafts/old.ts", ""),
    ] {
        let path = scripts.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let shown = [farmer("player1")];
    let mut app = App {
        names: vec!["player1".into()],
        picker: Some(
            Picker::new(
                library::Tab::Files,
                PathBuf::from("/srv/skua/Scripts"),
                Ok(None),
            )
            .with_files(scripts.clone()),
        ),
        ..App::default()
    };
    let s = screen(&app, &shown, 150, 30);
    let mut listed = s.lines().skip_while(|l| !l.contains("│ farm/ "));
    assert!(listed.next().is_some(), "folders come first:\n{s}");
    assert!(listed.next().unwrap().contains("│ quests.js "), "{s}");
    for skipped in ["types", "notes", "node_modules", ".drafts"] {
        assert!(!s.contains(skipped), "{skipped}:\n{s}");
    }
    let footer = s.lines().last().unwrap();
    assert!(
        footer.contains("← → change folder") && footer.contains("tab Skua scripts"),
        "{s}"
    );

    let source = Acts(RefCell::new(Vec::new()));
    app.handle(KeyEvent::from(KeyCode::Tab), &source, &shown);
    let s = screen(&app, &shown, 150, 30);
    assert!(
        s.contains("aqx hasn't downloaded the Skua scripts yet."),
        "{s}"
    );
    assert_eq!(app.picker_tab, library::Tab::Skua);
    app.handle(KeyEvent::from(KeyCode::BackTab), &source, &shown);

    let highlighted = |app: &App| {
        let picker = app.picker.as_ref().unwrap();
        (
            picker.folder().to_path_buf(),
            picker.shown()[picker.cursor].name.clone(),
        )
    };
    app.handle(KeyEvent::from(KeyCode::Right), &source, &shown);
    assert_eq!(
        highlighted(&app),
        (scripts.join("farm"), "frog-hunt.ts".into())
    );
    app.handle(KeyEvent::from(KeyCode::Left), &source, &shown);
    assert_eq!(highlighted(&app), (scripts.clone(), "farm/".into()));
    app.handle(KeyEvent::from(KeyCode::Left), &source, &shown);
    assert_eq!(
        highlighted(&app),
        (dir.clone(), "scripts/".into()),
        "the list isn't limited to the scripts folder"
    );
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);
    assert!(source.0.borrow().is_empty(), "enter on a folder opens it");

    app.handle(KeyEvent::from(KeyCode::Char('/')), &source, &shown);
    app.paste("farm", &shown);
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);
    app.handle(KeyEvent::from(KeyCode::Char('/')), &source, &shown);
    app.paste("frog", &shown);
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);
    assert!(app.picker.is_none());
    assert!(source.0.borrow().is_empty(), "the run waits for its args");
    app.paste("Frogzard", &shown);
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);

    let run = Action::Run(crate::config::Script {
        path: scripts.join("farm/frog-hunt.ts"),
        args: vec![("monster".into(), "Frogzard".into())],
    });
    assert_eq!(*source.0.borrow(), [(0, run)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_typed_or_pasted_path_lists_that_folder_and_runs_its_script() {
    let dir = std::env::temp_dir().join(format!("aqx-picker-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bots = dir.join("my bots (old)");
    std::fs::create_dir_all(bots.join("farm")).unwrap();
    for file in ["frog.ts", "fish.ts", "notes.txt"] {
        std::fs::write(bots.join(file), "").unwrap();
    }
    let shown = [farmer("player1")];
    let mut app = App {
        names: vec!["player1".into()],
        picker: Some(
            Picker::new(
                library::Tab::Skua,
                PathBuf::from("/srv/skua/Scripts"),
                Ok(None),
            )
            .with_files(dir.join("scripts")),
        ),
        ..App::default()
    };
    let source = Acts(RefCell::new(Vec::new()));
    let listed = |app: &App| {
        let picker = app.picker.as_ref().unwrap();
        let names: Vec<String> = picker.shown().iter().map(|e| e.name.clone()).collect();
        (picker.tab, picker.folder().to_path_buf(), names)
    };

    let dragged = format!("{}/", bots.display())
        .replace(' ', "\\ ")
        .replace('(', "\\(")
        .replace(')', "\\)");
    app.paste(&dragged, &shown);
    assert_eq!(
        listed(&app),
        (
            library::Tab::Files,
            bots.clone(),
            vec!["farm/".into(), "fish.ts".into(), "frog.ts".into()]
        ),
        "a path pasted on the Skua tab lists its folder on the files tab"
    );
    let s = screen(&app, &shown, 150, 30);
    assert!(
        s.contains("fish.ts"),
        "a path lists its folder from an empty one:\n{s}"
    );
    app.paste("FR", &shown);
    assert_eq!(
        listed(&app).2,
        ["frog.ts"],
        "the last part filters by its start"
    );
    app.handle(KeyEvent::from(KeyCode::Esc), &source, &shown);
    assert_eq!(listed(&app).1, dir.join("scripts"));

    app.handle(KeyEvent::from(KeyCode::Char('/')), &source, &shown);
    app.paste("../my bots (old)/far", &shown);
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);
    assert_eq!(
        listed(&app),
        (library::Tab::Files, bots.join("farm"), vec![]),
        "enter opens a folder"
    );

    app.paste(&format!("'{}'", bots.join("fish.ts").display()), &shown);
    app.handle(KeyEvent::from(KeyCode::Enter), &source, &shown);
    let run = Action::Run(crate::config::Script {
        path: bots.join("fish.ts"),
        args: Vec::new(),
    });
    assert_eq!(*source.0.borrow(), [(0, run)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_skua_picker_offers_the_download_before_the_first_one() {
    let mut picker = Picker::new(
        library::Tab::Skua,
        PathBuf::from("/srv/skua/Scripts"),
        Ok(None),
    );
    picker.failed = Some(
        "/srv/skua/Scripts wasn't downloaded by aqx, so aqx won't replace it. Move it somewhere else, then update again".into(),
    );
    let mut app = App {
        picker: Some(picker),
        ..App::default()
    };
    let s = screen(&app, &[farmer("player1")], 150, 30);
    assert!(s.contains("then update again"), "{s}");
    assert!(
        s.contains("aqx hasn't downloaded the Skua scripts yet."),
        "{s}"
    );
    assert!(
        s.contains("u downloads them from github.com/auqw/Scripts"),
        "{s}"
    );
    let picker = app.picker.as_mut().unwrap();
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Char('u'))),
        library::Outcome::Update
    );
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Enter)),
        library::Outcome::Nothing
    );
    assert_eq!(
        picker.key(KeyEvent::from(KeyCode::Esc)),
        library::Outcome::Close
    );
}

#[test]
fn a_skua_update_shows_its_progress_then_its_result() {
    let shown = [farmer("player1")];
    let footer = |app: &App| {
        screen(app, &shown, 150, 30)
            .lines()
            .last()
            .unwrap()
            .to_owned()
    };
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let (staged, reached) = std::sync::mpsc::channel::<()>();
    let mut app = App {
        update: Some(Update::spawn(move |stage| {
            stage("building the Skua worker");
            staged.send(()).unwrap();
            wait.recv().unwrap();
            Ok("updated the Skua scripts to 6c58f70".into())
        })),
        ..App::default()
    };
    reached.recv().unwrap();
    app.poll_update();
    assert!(
        footer(&app).contains("building the Skua worker"),
        "{}",
        footer(&app)
    );

    release.send(()).unwrap();
    let started = Instant::now();
    while app.update.is_some() && started.elapsed() < Duration::from_secs(5) {
        app.poll_update();
    }
    assert!(
        footer(&app).contains("updated the Skua scripts to 6c58f70"),
        "{}",
        footer(&app)
    );
}

#[test]
fn the_key_list_shows_every_label_whole_at_any_width() {
    let shown = [farmer("player1")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('?'));
    for (width, height) in [(80, 40), (50, 50)] {
        let s = screen(&app, &shown, width, height);
        for (_, rows) in draw::KEY_GROUPS {
            for &(commands, label) in rows {
                let keys = app.keymap.keys(commands);
                assert!(
                    row(&s, &format!("{keys:<9}  {label}")).contains(label),
                    "{keys} {label:?} at {width} wide:\n{s}"
                );
            }
        }
    }
    let s = screen(&app, &shown, 80, 40);
    assert!(row(&s, "1-8").contains("log and lists"), "{s}");
    assert!(row(&s, "PgUp PgDn").contains("scroll a page"), "{s}");
    press(&mut app, &shown, KeyCode::Char('x'));
    assert!(!app.keys, "any key closes the list");
}

#[test]
fn e_in_the_key_list_changes_a_key_and_saves_it() {
    let dir = std::env::temp_dir().join(format!("aqx-key-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("keys.toml");
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App {
        keymap: Keymap::load(path.clone()).unwrap(),
        ..App::default()
    };
    press(&mut app, &shown, KeyCode::Char('?'));
    let s = screen(&app, &shown, 100, 40);
    assert!(s.contains("Press e to change keys."), "{s}");
    press(&mut app, &shown, KeyCode::Char('e'));
    let s = screen(&app, &shown, 100, 40);
    assert!(
        row(&s, "Switch between the account list").contains("tab"),
        "{s}"
    );
    assert!(s.contains("enter change"), "{s}");
    app.handle(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &idle_fleet(),
        &shown,
    );
    press(&mut app, &shown, KeyCode::Esc);
    assert!(
        !app.ending && app.rebind.is_some(),
        "esc answers the end-session question over the editor"
    );
    assert!(
        s.contains("keys.toml."),
        "the note wraps instead of cutting the path:\n{s}"
    );
    let lines: Vec<&str> = s.lines().collect();
    let top = lines
        .iter()
        .position(|l| l.contains("┌ change keys"))
        .unwrap();
    let tall = lines[top..].iter().position(|l| l.contains("└──")).unwrap() + 1;
    assert!(tall <= 20, "the dialog stays short on a tall screen:\n{s}");

    let stop = keymap::COMMANDS
        .iter()
        .position(|e| e.name == "stop")
        .unwrap();
    for _ in 0..stop {
        press(&mut app, &shown, KeyCode::Down);
    }
    press(&mut app, &shown, KeyCode::Up);
    let s = screen(&app, &shown, 100, 40);
    assert!(
        s.contains("Stop the script."),
        "moving up inside the list doesn't scroll it:\n{s}"
    );
    press(&mut app, &shown, KeyCode::Down);
    press(&mut app, &shown, KeyCode::Enter);
    let s = screen(&app, &shown, 100, 40);
    assert!(row(&s, "Stop the script.").contains('…'), "{s}");
    press(&mut app, &shown, KeyCode::Char('r'));
    let s = screen(&app, &shown, 100, 40);
    assert!(s.contains("r is bound to both stop and restart"), "{s}");
    assert!(!path.exists(), "a clash saves nothing");

    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Char('k'));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "stop = \"k\"\n");
    assert!(
        accented(&app, &shown, 100, 40, "Stop the script.").starts_with('k'),
        "a changed key shows in blue"
    );
    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Esc);
    assert_eq!(app.keymap.key(Command::Stop), "k", "esc cancels a change");
    press(&mut app, &shown, KeyCode::Esc);
    assert!(
        app.rebind.is_none() && app.keys,
        "esc goes back to the key list"
    );
    press(&mut app, &shown, KeyCode::Esc);
    assert_eq!(
        app.keymap.command(KeyEvent::from(KeyCode::Char('k'))),
        Some(Command::Stop)
    );

    press(&mut app, &shown, KeyCode::Char('?'));
    press(&mut app, &shown, KeyCode::Char('e'));
    for _ in 0..stop {
        press(&mut app, &shown, KeyCode::Down);
    }
    press(&mut app, &shown, KeyCode::Backspace);
    assert_eq!(
        app.keymap.key(Command::Stop),
        "s",
        "backspace restores the default"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rebound_keys_drive_the_tui_and_show_where_keys_are_listed() {
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App {
        keymap: Keymap::parse(
            "up = [\"k\", \"up\"]\ndown = \"j\"\nview_drops = \"D\"\ndetails = []\nhelp = \"h\"\n",
        )
        .unwrap(),
        ..App::default()
    };
    press(&mut app, &shown, KeyCode::Char('j'));
    assert_eq!(app.selected, 1);
    press(&mut app, &shown, KeyCode::Down);
    assert_eq!(app.selected, 1, "down is no longer bound");
    press(&mut app, &shown, KeyCode::Char('k'));
    assert_eq!(app.selected, 0);
    press(&mut app, &shown, KeyCode::Char('d'));
    assert!(!app.details, "an empty list unbinds a command");

    let s = screen(&app, &shown, 150, 34);
    assert!(
        s.contains("│ D Drops 8"),
        "the side panel labels a list with its key:\n{s}"
    );
    let s = screen(&app, &shown, 80, 34);
    assert!(
        s.lines().last().unwrap().trim_end().ends_with("h more"),
        "a narrow footer points at the rebound key list:\n{s}"
    );
    press(&mut app, &shown, KeyCode::Char('h'));
    let s = screen(&app, &shown, 150, 40);
    assert!(row(&s, "k j ← →").contains("move"), "{s}");
    assert!(s.contains("1 2 D 4 5 6 7 8"), "{s}");
}

fn lit_boxes(app: &App, shown: &[Shown]) -> Vec<String> {
    let top = &cells(app, shown, 150, 30)[1];
    let text: Vec<&str> = top.iter().map(|c| c.symbol()).collect();
    text.iter()
        .enumerate()
        .filter(|&(x, &c)| c == "╭" && top[x].fg == Color::Blue)
        .map(|(x, _)| {
            text[x + 2..]
                .iter()
                .take_while(|&&c| c != " ")
                .copied()
                .collect()
        })
        .collect()
}

#[test]
fn tab_moves_focus_and_opening_drops_takes_it() {
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App::default();
    assert_eq!(
        lit_boxes(&app, &shown),
        ["accounts"],
        "the account list starts focused"
    );
    press(&mut app, &shown, KeyCode::Char(' '));
    assert_eq!(app.marked, BTreeSet::from([0]));

    press(&mut app, &shown, KeyCode::Char('3'));
    assert!(app.pane_focus, "the drops list takes focus to pick from");
    assert_eq!(lit_boxes(&app, &shown), ["player1"]);
    press(&mut app, &shown, KeyCode::Char(' '));
    press(&mut app, &shown, KeyCode::Down);
    assert_eq!(
        (app.picked.len(), app.cursor, app.selected),
        (1, 1, 0),
        "keys act on the drops, not the accounts"
    );

    press(&mut app, &shown, KeyCode::Esc);
    press(&mut app, &shown, KeyCode::Esc);
    assert!(
        app.view == View::Log && !app.pane_focus,
        "esc leaves the pane"
    );
    assert_eq!(app.marked, BTreeSet::from([0]));
    press(&mut app, &shown, KeyCode::Esc);
    assert!(app.marked.is_empty(), "then unmarks");

    press(&mut app, &shown, KeyCode::Enter);
    assert!(app.pane_focus, "enter focuses the main pane");
    press(&mut app, &shown, KeyCode::Tab);
    assert!(!app.pane_focus);
}

#[test]
fn slash_filters_the_sidebar_while_it_has_focus() {
    let shown = [farmer("player1"), farmer("player2")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('/'));
    type_in(&mut app, &shown, "player2");
    let s = screen(&app, &shown, 150, 30);
    assert!(s.contains(" filter accounts: player2"), "{s}");
    assert_eq!(app.selected, 1);
    press(&mut app, &shown, KeyCode::Enter);
    press(&mut app, &shown, KeyCode::Tab);
    press(&mut app, &shown, KeyCode::Char('/'));
    assert!(
        app.query.value().is_empty() && app.query_scope == Scope::Log,
        "the main pane starts its own search"
    );
}

#[test]
fn arrows_move_between_cards_on_the_dashboard() {
    let shown = [farmer("player1"), farmer("player2"), farmer("player3")];
    let mut app = App::default();
    press(&mut app, &shown, KeyCode::Char('g'));
    let _ = screen(&app, &shown, 150, 30);
    press(&mut app, &shown, KeyCode::Right);
    press(&mut app, &shown, KeyCode::Char(' '));
    press(&mut app, &shown, KeyCode::Tab);
    assert_eq!(
        (app.selected, app.marked.clone(), app.pane_focus),
        (1, BTreeSet::from([1]), false)
    );
    press(&mut app, &shown, KeyCode::Enter);
    assert!(!app.grid, "enter opens the account");
}

#[test]
fn switching_servers_relogs_the_marked_accounts_onto_an_open_server() {
    let shown = [farmer("player1"), farmer("player2")];
    let source = Acts(RefCell::new(Vec::new()));
    let mut app = App {
        names: vec!["player1".into(), "player2".into()],
        marked: BTreeSet::from([0, 1]),
        ..App::default()
    };
    let key = |app: &mut App, code| app.handle(KeyEvent::from(code), &source, &shown);
    let footer = |app: &App| {
        screen(app, &shown, 150, 30)
            .lines()
            .last()
            .unwrap()
            .to_owned()
    };
    key(&mut app, KeyCode::Char('c'));
    let s = screen(&app, &shown, 150, 30);
    assert!(
        row(&s, "900/1000").contains("Twilly       900/1000  current"),
        "{s}"
    );
    assert!(row(&s, "Gravelyn").contains("full"), "{s}");
    assert!(row(&s, "Sir Ver").contains("members"), "{s}");
    assert!(row(&s, "Yorumi").contains("offline"), "{s}");
    assert!(
        footer(&app).contains("switch player1, player2   ↑↓ pick  enter switch  esc close"),
        "{}",
        footer(&app)
    );

    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.servers.is_some(), "a full server keeps the list open");
    assert!(source.0.borrow().is_empty());
    assert!(screen(&app, &shown, 150, 30).contains("Gravelyn is full"));

    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.servers.is_none());
    assert_eq!(
        *source.0.borrow(),
        [
            (0, Action::Switch("Sir Ver".into())),
            (1, Action::Switch("Sir Ver".into())),
        ]
    );
    assert!(
        app.note
            .as_ref()
            .is_some_and(|(note, _)| note.contains("switching to Sir Ver")),
        "{:?}",
        app.note
    );

    key(&mut app, KeyCode::Char('c'));
    key(&mut app, KeyCode::Esc);
    assert!(app.servers.is_none());
    assert_eq!(source.0.borrow().len(), 2, "esc switches nothing");
}
