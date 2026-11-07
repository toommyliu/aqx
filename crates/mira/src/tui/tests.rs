use std::time::Duration;

use mira_client::ItemId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::style::Color;

use super::*;
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
                    name: "Spider Hunt".into(),
                    have: 4,
                    need: 10,
                },
                QuestProgress {
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
            "── log  main · all · mira · script · server · chat · game"
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
    press(&mut app, &shown, KeyCode::Char('B'));
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
    let lines: Vec<&str> = s.lines().skip(1).take(4).collect();
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
    press(&mut app, &shown, KeyCode::Char('}'));
    assert_eq!(app.selected, 0, "}} jumps to the next group");
    click(&mut app, &shown, 150, 30, "player2");
    assert_eq!(app.selected, 1);
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
    assert!(before.contains("jump to bottom (end)"), "{before}");
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
    assert!(after.contains("1 new · jump to bottom (end)"), "{after}");
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
    assert!(s.contains("End the mira session?"), "{s}");
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
        (Channel::Mira, "logging in to Twilly"),
        (Channel::Script, "farming"),
        (Channel::Mira, "the script stopped: done"),
        (Channel::Game, "killed Frogzard, +10 gold, +5 xp"),
        (Channel::Mira, "disconnected: logged out"),
    ] {
        accounts[0].board().log.push(channel, text.into());
    }
    accounts[1]
        .board()
        .log
        .push(Channel::Mira, "couldn't log in: player2 is banned".into());
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
        redacted[3].contains("account 2  mira    couldn't log in: account 2 is banned"),
        "{redacted:?}"
    );

    let one = Fleet::of(accounts[1..].to_vec());
    assert!(
        !last_words(&one.last_lines(), false)[0].contains("player2  mira"),
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
    let home = std::env::temp_dir().join(format!("mira-ask-tests-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("follow.ts"),
        "import { script } from \"mira\";\nexport default function* () { script.args<{ leader: string }>(); }\n",
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
        "import { script } from \"mira\";\nexport default function* () { script.args<{ wait?: number; class?: string }>({ wait: 5, class: \"\" }); }\n",
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
    press(&mut app, &shown, KeyCode::Char('o'));
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
    let home = std::env::temp_dir().join(format!("mira-paste-tests-{}", std::process::id()));
    std::fs::create_dir_all(home.join("farm")).unwrap();
    std::fs::write(
        home.join("farm/follow.ts"),
        "import { script } from \"mira\";\nexport default function* () { script.args<{ leader: string }>(); }\n",
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

    press(&mut app, &shown, KeyCode::Char('o'));
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
