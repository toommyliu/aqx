use std::ops::Range;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use tui_input::Input as TextInput;
use unicode_width::UnicodeWidthChar;

use super::keymap::Command;
use super::library::{Catalog, Picker, Tab as PickerTab};
use super::skua::{Pane, Skua, Tab};
use super::{App, Filter, Hit, Scope, Section, View};
use crate::fleet::{Channel, Line as LogLine};
use crate::view::{Game, QuestProgress, Shown, Stack, Standing, State, found, matches, order};
use mira_script::skua::library;

const WIDE: u16 = 90;
const ACCENT: Color = Color::Blue;
const FOUND: Style = Style::new().fg(Color::Black).bg(Color::Yellow);
const CARD_WIDTH: u16 = 44;
const CARD_HEIGHT: u16 = 8;
const RAIL_WIDTH: u16 = 34;
const LOG_MIN: u16 = 76;
const COLUMN_MIN: usize = 20;
const COLUMN_MAX: usize = 40;
const SIDEBAR_MIN: u16 = 10;
const ABOUT_WIDTH: usize = 14;
const PANE_MIN: u16 = 40;
const LABEL_MIN: usize = 6;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

type Field = Vec<Span<'static>>;

type KeyRow = (&'static [Command], &'static str);

pub(super) const KEY_GROUPS: [(&str, &[KeyRow]); 4] = [
    (
        "In the focused pane",
        &[
            (&[Command::Focus], "switch pane"),
            (
                &[Command::Up, Command::Down, Command::Left, Command::Right],
                "move",
            ),
            (&[Command::Mark], "mark"),
            (&[Command::MarkAll], "mark all / none"),
            (&[Command::Open], "open, accept drops"),
            (&[Command::Search], "search"),
            (&[Command::Back], "back, unmark"),
        ],
    ),
    (
        "Accounts",
        &[
            (&[Command::LogIn], "log in"),
            (&[Command::LogOut], "log out"),
            (&[Command::PickScript], "pick a script"),
            (&[Command::TypeScript], "type a script"),
            (&[Command::Stop, Command::Restart], "stop / restart"),
            (&[Command::Pause], "pause / resume"),
        ],
    ),
    (
        "View",
        &[
            (&VIEWS, "log and lists"),
            (
                &[Command::PreviousChannel, Command::NextChannel],
                "log channel",
            ),
            (&[Command::Dashboard], "dashboard"),
            (&[Command::Details], "details"),
            (&[Command::Zoom], "zoom"),
            (
                &[Command::NarrowerSidebar, Command::WiderSidebar],
                "sidebar width",
            ),
            (&[Command::PageUp, Command::PageDown], "scroll a page"),
            (&[Command::Oldest, Command::Newest], "oldest / newest"),
        ],
    ),
    (
        "Session",
        &[
            (&[Command::Help], "keys"),
            (&[Command::Detach], "detach"),
            (&[Command::EndSession], "end the session"),
        ],
    ),
];
const KEY_GAP: usize = 4;
const VIEWS: [Command; 8] = [
    Command::View(View::Log),
    Command::View(View::List(Section::Players)),
    Command::View(View::List(Section::Drops)),
    Command::View(View::List(Section::Quests)),
    Command::View(View::List(Section::Temp)),
    Command::View(View::List(Section::Inventory)),
    Command::View(View::List(Section::Bank)),
    Command::View(View::List(Section::Factions)),
];

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Players => "Players",
            Section::Drops => "Drops",
            Section::Quests => "Quests",
            Section::Temp => "Temp",
            Section::Inventory => "Inventory",
            Section::Bank => "Bank",
            Section::Factions => "Factions",
        }
    }

    fn rail_rows(self) -> usize {
        match self {
            Section::Players | Section::Quests | Section::Temp | Section::Inventory => 4,
            Section::Factions => 3,
            Section::Drops => 6,
            Section::Bank => 0,
        }
    }

    fn on_rail(self, g: &Game) -> usize {
        match self {
            Section::Factions => g.factions.iter().filter(|f| f.gained).count(),
            Section::Bank => 0,
            _ => self.len(g),
        }
    }

    fn len(self, g: &Game) -> usize {
        match self {
            Section::Players => g.players.len(),
            Section::Drops => g.drops.len(),
            Section::Quests => g.quests.len(),
            Section::Factions => g.factions.len(),
            Section::Temp => g.temp.len(),
            Section::Inventory => g.inventory.len(),
            Section::Bank => g.bank.as_ref().map_or(0, Vec::len),
        }
    }

    fn count(self, g: &Game) -> Field {
        match self {
            Section::Inventory if g.misc_used > 0 => vec![
                tally(g.bag_used, g.bag_slots),
                Span::raw(" · misc "),
                tally(g.misc_used, g.bag_slots),
            ],
            Section::Inventory => vec![tally(g.bag_used, g.bag_slots)],
            Section::Bank => vec![tally(g.bank_used, g.bank_slots)],
            _ => vec![Span::raw(self.len(g).to_string())],
        }
    }

    fn order(self) -> Option<&'static str> {
        match self {
            Section::Players => None,
            Section::Quests => Some("in the order accepted"),
            Section::Factions => Some("gained first, then by name"),
            Section::Inventory => Some("equipped first, then last changed"),
            _ => Some("last changed first"),
        }
    }

    fn entries(self, g: &Game) -> Vec<(String, Field)> {
        let stacks = |stacks: &[Stack]| stacks.iter().map(|s| (s.name.clone(), held(s))).collect();
        match self {
            Section::Players => g
                .players
                .iter()
                .map(|p| {
                    let level = p.level.map(|l| l.to_string()).unwrap_or_default();
                    (p.name.clone(), vec![Span::raw(level)])
                })
                .collect(),
            Section::Drops => g
                .drops
                .iter()
                .map(|d| (d.name.clone(), vec![Span::raw(d.qty.to_string())]))
                .collect(),
            Section::Quests => g
                .quests
                .iter()
                .map(|q| (q.name.clone(), turn_in(q)))
                .collect(),
            Section::Factions => g
                .factions
                .iter()
                .map(|f| (f.name.clone(), standing(f)))
                .collect(),
            Section::Temp => stacks(&g.temp),
            Section::Inventory => stacks(&g.inventory),
            Section::Bank => g.bank.as_deref().map(stacks).unwrap_or_default(),
        }
    }

    fn nothing(self, g: &Game) -> &'static str {
        match self {
            Section::Players => "Nobody else is in this cell.",
            Section::Drops => "No drops are waiting.",
            Section::Quests => "No quests are accepted.",
            Section::Factions => "No factions joined yet.",
            Section::Temp => "No temporary items.",
            Section::Inventory => "The inventory is empty.",
            Section::Bank if g.bank.is_none() => "The bank shows here once a script opens it.",
            Section::Bank => "The bank is empty.",
        }
    }
}

enum Mark {
    Spin,
    Still(&'static str),
}

struct Status {
    mark: Mark,
    color: Color,
    word: &'static str,
    about: String,
}

impl Status {
    fn of(s: &Shown) -> Self {
        let word = s.state.word();
        let (mark, color, about) = match &s.state {
            State::Busy(_) => (Mark::Spin, Color::Yellow, word.to_owned()),
            State::Reconnecting(detail) => {
                (Mark::Spin, Color::Yellow, format!("reconnecting {detail}"))
            }
            State::Running => (Mark::Spin, Color::Cyan, script_label(s)),
            State::Stalled => (
                Mark::Still("!"),
                Color::Yellow,
                format!("{}, stalled", script_label(s)),
            ),
            State::Paused => (
                Mark::Still("‖"),
                Color::Yellow,
                format!("{}, paused", script_label(s)),
            ),
            State::Online if s.script.is_some() => {
                (Mark::Still("●"), Color::Green, script_label(s))
            }
            State::Online => (Mark::Still("●"), Color::Green, "idle".to_owned()),
            State::Offline { why: None } => {
                (Mark::Still("○"), Color::DarkGray, "logged out".to_owned())
            }
            State::Offline { why: Some(why) } => (Mark::Still("✗"), Color::Red, why.clone()),
        };
        Status {
            mark,
            color,
            word,
            about,
        }
    }

    fn mark(&self, frame: usize) -> Span<'static> {
        let glyph = match self.mark {
            Mark::Spin => SPINNER[frame % SPINNER.len()],
            Mark::Still(glyph) => glyph,
        };
        Span::raw(glyph).fg(self.color)
    }

    fn badge(&self, frame: usize) -> Field {
        vec![
            self.mark(frame),
            Span::raw(format!(" {}", self.word)).fg(self.color).bold(),
        ]
    }
}

fn script_label(s: &Shown) -> String {
    let Some(script) = &s.script else {
        return String::new();
    };
    let name = script.rsplit('/').next().unwrap_or(script);
    if s.running() || s.status.is_empty() {
        name.to_owned()
    } else {
        format!("{name}, {}", s.status)
    }
}

pub(super) fn draw(f: &mut Frame, app: &App, shown: &[Shown]) {
    app.hits.borrow_mut().clear();
    let matches: Vec<String> = match &app.prompt {
        _ if app.querying && app.query_scope == Scope::Accounts => order(shown, app.query.value())
            .into_iter()
            .map(|i| shown[i].name.clone())
            .collect(),
        Some(prompt) => prompt.matches.clone(),
        None => Vec::new(),
    };
    let [header, body, suggestions, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(u16::from(!matches.is_empty())),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_matches(f, suggestions, &matches);
    let sidebar = !app.grid && !app.zoomed && body.width >= WIDE && shown.len() > 1;
    app.sidebar.set(None);
    draw_header(f, header, app, shown, sidebar || app.grid);
    if app.grid {
        draw_grid(f, body, app, shown);
    } else if let Some(account) = shown.get(app.selected) {
        let pane = if sidebar {
            let fitted = (4 + name_width(shown) + 1 + ABOUT_WIDTH + 1) as u16;
            let width = app
                .sidebar_width
                .unwrap_or(fitted)
                .clamp(SIDEBAR_MIN, body.width - PANE_MIN);
            let [left, _, pane, _] = Layout::horizontal([
                Constraint::Length(width),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .areas(body);
            app.sidebar.set(Some(left));
            draw_sidebar(f, left, app, shown);
            pane
        } else {
            body.inner(Margin::new(1, 0))
        };
        draw_account(f, pane, account, app);
    }
    draw_footer(f, footer, app, shown);
    if let Some(picker) = &app.picker {
        draw_picker(f, body, app, picker);
    }
    if let Some(skua) = &app.skua {
        draw_skua(f, body, app, skua, shown.get(skua.account));
    }
    if app.keys {
        draw_keys(f, body, app);
    }
    if app.ending {
        draw_ending(f, body, shown.iter().filter(|s| !s.offline()).count());
    }
}

fn hit(app: &App, rect: Rect, hit: Hit) {
    app.hits.borrow_mut().push((rect, hit));
}

fn draw_matches(f: &mut Frame, area: Rect, matches: &[String]) {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for (i, path) in matches.iter().enumerate() {
        let name = &path[path.trim_end_matches('/').rfind('/').map_or(0, |at| at + 1)..];
        let span = if name.ends_with('/') {
            Span::raw(name.to_owned()).blue()
        } else {
            Span::raw(name.to_owned())
        };
        let gap = if i == 0 { "" } else { "  " };
        let after = matches.len() - i - 1;
        let reserve = if after == 0 {
            0
        } else {
            format!("  +{after} more").len()
        };
        if used + gap.len() + span.width() + reserve > usize::from(area.width) {
            spans.push(Span::raw(format!("{gap}+{} more", matches.len() - i)).dark_gray());
            break;
        }
        used += gap.len() + span.width();
        spans.extend([Span::raw(gap), span]);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App, shown: &[Shown], overview: bool) {
    let mut left = vec![Span::raw(" mira").bold()];
    if overview {
        let online = shown.iter().filter(|s| !s.offline()).count();
        left.push(Span::raw(format!("  {online} of {} online", shown.len())).dark_gray());
    } else if let Some(account) = shown.get(app.selected).filter(|_| shown.len() > 1) {
        let order = order(shown, app.search(Scope::Accounts));
        let at = order.iter().position(|&i| i == app.selected).unwrap_or(0);
        left.push(Span::raw(format!("  {} {}/{}", account.name, at + 1, order.len())).dark_gray());
    }
    let accounts = app.search(Scope::Accounts);
    if !accounts.is_empty() {
        left.push(Span::raw(format!(" · filter \"{accounts}\"")).dark_gray());
    }
    if !app.marked.is_empty() {
        left.push(Span::raw(format!(" · {} marked", app.marked.len())).dark_gray());
    }
    let key = |c| app.keymap.key(c);
    let mut keys = Vec::new();
    if !app.grid && shown.len() > 1 {
        keys.push((key(Command::Focus), "switch pane"));
    }
    if shown.len() > 1 {
        let search = if app.on_pane(shown) {
            "search"
        } else {
            "filter"
        };
        keys.extend([
            (key(Command::Dashboard), "dashboard"),
            (key(Command::Search), search),
        ]);
    }
    keys.extend([
        (key(Command::Help), "keys"),
        (key(Command::Detach), "detach"),
    ]);
    let width = area.width.saturating_sub(1) as usize;
    f.render_widget(Paragraph::new(spread(left, hints(&keys), width)), area);
}

fn name_width(shown: &[Shown]) -> usize {
    shown
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(4, 14)
}

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let inner = usize::from(area.width.saturating_sub(1));
    let name_col = name_width(shown).min(inner.saturating_sub(4));
    let rows = sidebar_rows(shown, app.search(Scope::Accounts));
    let offline = rows
        .iter()
        .flatten()
        .filter(|&&i| shown[i].offline())
        .count();
    let height = (area.height as usize).max(1);
    let at = rows
        .iter()
        .position(|&row| row == Some(app.selected))
        .unwrap_or(0);
    let top = app
        .sidebar_top
        .get()
        .min(at)
        .max((at + 1).saturating_sub(height));
    app.sidebar_top.set(top);
    let mut lines = Vec::new();
    for (y, &row) in rows.iter().skip(top).take(height).enumerate() {
        lines.push(match row {
            Some(i) => {
                let rect = Rect {
                    y: area.y + y as u16,
                    height: 1,
                    width: area.width.saturating_sub(1),
                    ..area
                };
                hit(app, rect, Hit::Account(i));
                sidebar_row(
                    &shown[i],
                    i == app.selected,
                    app.marked.contains(&i),
                    app.frame,
                    name_col,
                    inner,
                )
            }
            None => {
                let label = format!("── offline {offline} ");
                let fill = inner.saturating_sub(label.chars().count());
                fit(
                    vec![Span::raw(format!("{label}{}", "─".repeat(fill))).dark_gray()],
                    inner,
                )
            }
        });
    }
    let rule = Block::new()
        .borders(Borders::RIGHT)
        .border_style(if app.pane_focus {
            Style::new().dark_gray()
        } else {
            Style::new().fg(ACCENT)
        });
    f.render_widget(Paragraph::new(lines).block(rule), area);
}

fn sidebar_rows(shown: &[Shown], query: &str) -> Vec<Option<usize>> {
    let mut rows: Vec<Option<usize>> = order(shown, query).into_iter().map(Some).collect();
    let offline = rows
        .iter()
        .flatten()
        .filter(|&&i| shown[i].offline())
        .count();
    if 0 < offline && offline < rows.len() {
        rows.insert(rows.len() - offline, None);
    }
    rows
}

fn sidebar_row(
    account: &Shown,
    selected: bool,
    marked: bool,
    frame: usize,
    name_col: usize,
    inner: usize,
) -> Line<'static> {
    let status = Status::of(account);
    let name = Span::raw(account.name.clone());
    let mut spans = vec![
        Span::raw(if selected { "▌" } else { " " }).fg(ACCENT),
        Span::raw(if marked { "◆" } else { " " }).fg(ACCENT),
        status.mark(frame),
        Span::raw(" "),
    ];
    spans.extend(clip(
        vec![if selected { name.bold() } else { name }],
        name_col,
    ));
    if inner >= 4 + name_col + 1 + LABEL_MIN {
        spans.push(Span::raw(
            " ".repeat(4 + name_col + 1 - spans_width(&spans)),
        ));
        spans.push(Span::raw(status.about).dark_gray());
    }
    fit(spans, inner)
}

fn typed_spans(input: &TextInput) -> Field {
    let value = input.value();
    let at = value
        .char_indices()
        .nth(input.cursor())
        .map_or(value.len(), |(i, _)| i);
    let (before, after) = value.split_at(at);
    let mut after = after.chars();
    let cursor = match after.next() {
        Some(c) => Span::raw(c.to_string()).reversed(),
        None => Span::raw("█"),
    };
    vec![
        Span::raw(before.to_owned()),
        cursor,
        Span::raw(after.as_str().to_owned()),
    ]
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let width = area.width as usize;
    let who = app.who(shown, width / 3);
    if app.closing {
        let online = shown.iter().filter(|s| !s.offline()).count();
        let text = match online {
            0 => " ending the session".to_owned(),
            1 => " logging out 1 account; the session ends once it's offline".to_owned(),
            n => format!(" logging out {n} accounts; the session ends once they're offline"),
        };
        let line = vec![
            Span::raw(SPINNER[app.frame % SPINNER.len()]).yellow(),
            Span::raw(text).yellow(),
        ];
        return f.render_widget(Paragraph::new(Line::from(line)), area);
    }
    if app.querying {
        let (label, keys) = match app.query_scope {
            Scope::Accounts => (
                " filter accounts: ".to_owned(),
                hints(&[
                    ("↑↓", "pick"),
                    ("tab", "complete"),
                    ("enter", "done"),
                    ("esc", "clear"),
                ]),
            ),
            Scope::Log => (
                " search the log: ".to_owned(),
                hints(&[("enter", "done"), ("esc", "clear")]),
            ),
            Scope::List(section) => (
                format!(" search {}: ", section.label().to_lowercase()),
                hints(&[("enter", "done"), ("esc", "clear")]),
            ),
        };
        let typed = [vec![Span::raw(label).bold()], typed_spans(&app.query)].concat();
        return f.render_widget(
            Paragraph::new(spread(typed, keys, width.saturating_sub(1))),
            area,
        );
    }
    if let Some(skua) = &app.skua {
        let keys: &[(&str, &str)] = match (&skua.editing, skua.tab) {
            (Some(_), _) => &[("enter", "save"), ("esc", "cancel")],
            (None, Tab::CoreBots) => &[
                ("↑↓", "pick"),
                ("enter ← →", "change"),
                ("del", "use the default"),
                ("tab", "script options"),
                ("esc", "close"),
            ],
            (None, Tab::Script) => &[
                ("↑↓", "pick"),
                ("enter ← →", "change"),
                ("tab", "CoreBots options"),
                ("esc", "close"),
            ],
        };
        return f.render_widget(
            Paragraph::new(Line::from(clip(
                [vec![Span::raw(" ")], hints(keys)].concat(),
                width,
            ))),
            area,
        );
    }
    if let Some(picker) = &app.picker {
        let keys: &[(&str, &str)] = match (picker.searching, picker.tab) {
            (true, _) => &[("↑↓", "pick"), ("enter", "run"), ("esc", "clear")],
            (false, PickerTab::Mira) => &[
                ("↑↓", "pick"),
                ("enter", "run"),
                ("/", "search"),
                ("tab", "Skua scripts"),
                ("esc", "close"),
            ],
            (false, PickerTab::Skua) => &[
                ("↑↓", "pick"),
                ("enter", "run"),
                ("→", "options"),
                ("/", "search"),
                ("u", "update"),
                ("tab", "mira scripts"),
                ("esc", "close"),
            ],
        };
        let line = [
            vec![Span::raw(format!(" run on {who}   ")).bold()],
            hints(keys),
        ]
        .concat();
        return f.render_widget(Paragraph::new(Line::from(clip(line, width))), area);
    }
    if let Some(prompt) = &app.prompt {
        let keys = hints(&[("tab", "complete"), ("↑↓", "recent"), ("esc", "cancel")]);
        let typed = [
            vec![Span::raw(format!(" run on {who}: ")).bold()],
            typed_spans(&prompt.input),
        ]
        .concat();
        return f.render_widget(
            Paragraph::new(spread(typed, keys, width.saturating_sub(1))),
            area,
        );
    }
    if let Some(ask) = &app.ask {
        let script = ask.script.path.file_name().map_or_else(
            || ask.script.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let (arg, account) = ask.asking();
        let account = account.map(|a| format!(" for {a}")).unwrap_or_default();
        let mut keys = vec![("enter", if ask.left() > 1 { "next" } else { "run" })];
        if ask.can_share() {
            keys.push((
                "tab",
                if ask.shared {
                    "ask each account"
                } else {
                    "same for all"
                },
            ));
        }
        keys.push(("esc", "cancel"));
        let input = if ask.recalled {
            vec![
                Span::raw(ask.input.value().to_owned()).reversed(),
                Span::raw("█"),
            ]
        } else {
            typed_spans(&ask.input)
        };
        let label = format!(" {script} needs {}{account}: ", arg.name);
        let mut typed = [vec![Span::raw(label).bold()], input].concat();
        match &ask.error {
            Some(error) => typed.push(Span::raw(format!("  {error}")).red()),
            None if ask.input.value().is_empty() && arg.optional => {
                let kind = super::ask::kind(arg);
                let hint = match arg.default.as_deref().filter(|d| !d.is_empty()) {
                    Some(default) => format!("{kind}, enter uses {default}"),
                    None => format!("{kind}, optional: enter skips"),
                };
                typed.push(Span::raw(hint).dark_gray());
            }
            None if ask.input.value().is_empty() => {
                typed.push(Span::raw(super::ask::kind(arg)).dark_gray())
            }
            None => {}
        }
        return f.render_widget(
            Paragraph::new(spread(typed, hints(&keys), width.saturating_sub(1))),
            area,
        );
    }
    if let Some((note, until)) = &app.note
        && std::time::Instant::now() < *until
    {
        return f.render_widget(Paragraph::new(format!(" {note}")).yellow(), area);
    }
    if let Some(update) = &app.update {
        let line = vec![
            Span::raw(format!(" {} ", SPINNER[app.frame % SPINNER.len()])).yellow(),
            Span::raw(update.stage).yellow(),
        ];
        return f.render_widget(Paragraph::new(Line::from(line)), area);
    }
    let keys = if app.view != View::Log && !app.grid {
        list_keys(app, app.on_pane(shown))
    } else {
        account_keys(app, shown)
    };
    let mut line = vec![
        Span::raw(" → ").fg(ACCENT),
        Span::raw(who).bold(),
        Span::raw("   "),
    ];
    let used = spans_width(&line);
    if used + spans_width(&hints(&keys)) <= width {
        line.extend(hints(&keys));
    } else {
        let help = [(app.keymap.key(Command::Help), "more")];
        let more = spans_width(&hints(&help)) + 2;
        let fit = (0..keys.len())
            .rev()
            .find(|&n| used + spans_width(&hints(&keys[..n])) + more <= width)
            .unwrap_or(0);
        line.extend(hints(&keys[..fit]));
        if fit > 0 {
            line.push(Span::raw("  "));
        }
        line.extend(hints(&help));
    }
    f.render_widget(Paragraph::new(Line::from(line)), area);
}

fn list_keys(app: &App, pane: bool) -> Vec<(String, &'static str)> {
    let key = |c| app.keymap.key(c);
    let mut keys = Vec::new();
    if app.picking() && app.list_len.get() > 0 && !pane {
        keys.push((key(Command::Focus), "pick drops"));
    } else if app.picking() && app.list_len.get() > 0 {
        let accept = if app.picked.is_empty() {
            "accept"
        } else {
            "accept marked"
        };
        keys.extend([
            (key(Command::Up) + &key(Command::Down), "pick"),
            (key(Command::Mark), "mark"),
            (key(Command::MarkAll), "mark all"),
            (key(Command::Open), accept),
        ]);
    } else if app.list_max.get() > 0 {
        keys.push((
            app.keymap.keys(&[Command::PageUp, Command::PageDown]),
            "scroll",
        ));
    }
    keys.extend([
        (key(Command::Search), "search"),
        (app.keymap.keys(&VIEWS), "views"),
        (
            key(Command::Back),
            if !app.picked.is_empty() {
                "clear marks"
            } else if !app.query.value().is_empty() {
                "clear search"
            } else {
                "back to log"
            },
        ),
    ]);
    keys
}

fn account_keys(app: &App, shown: &[Shown]) -> Vec<(String, &'static str)> {
    let key = |c| app.keymap.key(c);
    let details = if app.details { "less" } else { "details" };
    let offline = app.all(shown, Shown::offline);
    let mut keys = if offline {
        vec![
            (key(Command::LogIn), "log in"),
            (key(Command::PickScript), "log in and run"),
        ]
    } else {
        let pause = if app.all(shown, Shown::paused) {
            "resume"
        } else {
            "pause"
        };
        vec![
            (key(Command::PickScript), "run"),
            (key(Command::Stop), "stop"),
            (key(Command::Restart), "restart"),
            (key(Command::Pause), pause),
            (key(Command::LogOut), "log out"),
        ]
    };
    if !app.on_pane(shown) {
        keys.push((key(Command::Mark), "mark"));
    }
    let channels = app
        .keymap
        .keys(&[Command::PreviousChannel, Command::NextChannel]);
    match (app.grid, offline) {
        (true, _) => keys.push((key(Command::Open), "open")),
        (false, true) => keys.extend([(key(Command::Details), details), (channels, "channel")]),
        (false, false) => keys.extend([
            (key(Command::Details), details),
            (channels, "channel"),
            (app.keymap.keys(&VIEWS[1..]), "lists"),
        ]),
    }
    keys
}

fn draw_grid(f: &mut Frame, area: Rect, app: &App, shown: &[Shown]) {
    let cols = usize::from(area.width / CARD_WIDTH).max(1);
    let rows = usize::from(area.height / CARD_HEIGHT).max(1);
    app.grid_cols.set(cols);
    let order = order(shown, app.search(Scope::Accounts));
    let row = order.iter().position(|&i| i == app.selected).unwrap_or(0) / cols;
    let top = app
        .grid_top
        .get()
        .min(row)
        .max((row + 1).saturating_sub(rows));
    app.grid_top.set(top);
    for (at, &i) in order.iter().skip(top * cols).take(rows * cols).enumerate() {
        let edge = |col: usize| (usize::from(area.width) * col / cols) as u16;
        let rect = Rect {
            x: area.x + edge(at % cols),
            y: area.y + (at / cols) as u16 * CARD_HEIGHT,
            width: edge(at % cols + 1) - edge(at % cols),
            height: CARD_HEIGHT,
        };
        let rect = rect.intersection(area);
        hit(app, rect, Hit::Account(i));
        draw_card(
            f,
            rect,
            &shown[i],
            i == app.selected,
            app.marked.contains(&i),
            app.frame,
        );
    }
}

fn draw_card(
    f: &mut Frame,
    area: Rect,
    account: &Shown,
    selected: bool,
    marked: bool,
    frame: usize,
) {
    let mut badge = vec![Span::raw(" ")];
    badge.extend(Status::of(account).badge(frame));
    badge.push(Span::raw(" "));
    let mut name = Vec::new();
    if marked {
        name.push(Span::raw(" ◆").fg(ACCENT));
    }
    name.push(Span::raw(format!(" {} ", account.name)).bold());
    let name = clip(
        name,
        usize::from(area.width).saturating_sub(spans_width(&badge) + 3),
    );
    let (border, color) = if selected {
        (BorderType::Thick, ACCENT)
    } else {
        (BorderType::Plain, Color::DarkGray)
    };
    let block = Block::bordered()
        .border_type(border)
        .border_style(Style::new().fg(color))
        .title(Line::from(name).left_aligned())
        .title(Line::from(badge).right_aligned())
        .padding(Padding::horizontal(1));
    let width = usize::from(area.width.saturating_sub(4));
    f.render_widget(Paragraph::new(card(account, width)).block(block), area);
}

fn card(s: &Shown, width: usize) -> Vec<Line<'static>> {
    let mut about = Vec::new();
    if !s.user.eq_ignore_ascii_case(&s.name) {
        about.push(s.user.clone());
    }
    if !s.server.is_empty() {
        about.push(s.server.clone());
    }
    if let Some(up) = s.up {
        about.push(format!("up {}", span(up)));
    }
    let place = match (trouble(s), &s.game) {
        (Some(t), _) => vec![t],
        (None, Some(g)) => vec![Span::raw(format!("{} {}/{}", g.area, g.cell, g.pad))],
        (None, None) => Vec::new(),
    };
    let mut lines: Vec<Line> = [
        vec![Span::raw(about.join(" · ")).dark_gray()],
        place,
        task(s),
    ]
    .into_iter()
    .map(|spans| fit(spans, width))
    .collect();
    if let Some(g) = &s.game {
        let mut fields = summary(s, g);
        let pools = [("inventory", g.bag_used), ("misc", g.misc_used)];
        fields.extend(pools.into_iter().filter_map(|(pool, used)| {
            fill(used, g.bag_slots).map(|(color, word)| {
                vec![Span::raw(format!("{pool} {used}/{} {word}", g.bag_slots)).fg(color)]
            })
        }));
        lines.extend(flow(vec![fields], width));
    }
    lines
}

fn trouble(s: &Shown) -> Option<Span<'static>> {
    match &s.state {
        State::Offline { why: Some(why) } => Some(Span::raw(why.clone()).red()),
        State::Reconnecting(detail) => Some(Span::raw(format!("reconnecting {detail}")).yellow()),
        _ => None,
    }
}

fn task(s: &Shown) -> Field {
    let online = !s.offline();
    let mut task = vec![match &s.script {
        Some(_) => Span::raw(script_label(s)).fg(if online && s.running() {
            Color::Cyan
        } else {
            Color::DarkGray
        }),
        None => Span::raw("idle").dark_gray(),
    }];
    if s.running() && !s.status.is_empty() {
        task.push(Span::raw(format!("  {}", s.status)));
    }
    task
}

fn draw_account(f: &mut Frame, area: Rect, account: &Shown, app: &App) {
    let head = detail(account, app.details, app.frame, usize::from(area.width));
    let [top, divider, below] = Layout::vertical([
        Constraint::Length(head.len() as u16),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(head), top);
    let game = account.game.as_ref();
    let rail = game.filter(|_| !app.zoomed && area.width >= LOG_MIN + RAIL_WIDTH);
    let side = if rail.is_some() { RAIL_WIDTH } else { 0 };
    let [main, side] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(side)]).areas(below);
    let mut rule = Strip::new(divider.x);
    rule.push(Span::raw("── "));
    let tabs = game.filter(|_| rail.is_none());
    match (app.view, game) {
        (View::List(section), Some(g)) => {
            draw_section(f, main, g, section, app);
            match tabs {
                Some(g) => tab_strip(&mut rule, app, g, usize::from(main.width)),
                None => {
                    rule.push(
                        Span::raw(format!(
                            "{} {} ",
                            app.keymap.key(Command::View(View::List(section))),
                            section.label().to_lowercase()
                        ))
                        .fg(ACCENT)
                        .bold(),
                    );
                    rule.extend(section.count(g));
                    if let Some(order) = section.order() {
                        rule.push(Span::raw(format!(" · {order}")));
                    }
                }
            }
            searched(&mut rule, app);
            if !app.picked.is_empty() {
                rule.push(Span::raw(format!(" · {} marked", app.picked.len())));
            }
            let (at, max) = (
                app.list_at.get().min(app.list_max.get()),
                app.list_max.get(),
            );
            if at > 0 {
                rule.push(Span::raw(format!(" · ↑{at}")));
            }
            if at < max {
                rule.push(Span::raw(format!(" · ↓{}", max - at)));
            }
        }
        _ => {
            let scrolled = draw_log(f, main, app);
            match tabs {
                Some(g) => tab_strip(&mut rule, app, g, usize::from(main.width)),
                None => filter_strip(&mut rule, app),
            }
            searched(&mut rule, app);
            if scrolled > 0 {
                rule.push(Span::raw(format!(" · ↑{scrolled}")));
            }
        }
    }
    rule.push(Span::raw(" "));
    let width = usize::from(main.width);
    rule.push(Span::raw("─".repeat(width.saturating_sub(rule.width()))));
    let mut spans = clip(rule.spans, width);
    for (x, w, target) in rule.hits {
        if x + w <= divider.x + main.width {
            hit(
                app,
                Rect {
                    x,
                    y: divider.y,
                    width: w,
                    height: 1,
                },
                target,
            );
        }
    }
    if let Some(g) = rail {
        spans.push(Span::raw(format!(
            "┬{}",
            "─".repeat(usize::from(RAIL_WIDTH) - 1)
        )));
        draw_rail(f, side, g, app);
    }
    f.render_widget(Paragraph::new(Line::from(spans)).dark_gray(), divider);
}

struct Strip {
    x: u16,
    spans: Field,
    hits: Vec<(u16, u16, Hit)>,
}

impl Strip {
    fn new(x: u16) -> Strip {
        Strip {
            x,
            spans: Vec::new(),
            hits: Vec::new(),
        }
    }

    fn width(&self) -> usize {
        spans_width(&self.spans)
    }

    fn push(&mut self, span: Span<'static>) {
        self.spans.push(span);
    }

    fn extend(&mut self, spans: Field) {
        self.spans.extend(spans);
    }

    fn target(&mut self, spans: Field, target: Hit) {
        let x = self.x + self.width() as u16;
        let w = spans_width(&spans) as u16;
        self.spans.extend(spans);
        self.hits.push((x, w, target));
    }
}

fn searched(rule: &mut Strip, app: &App) {
    if !app.query.value().is_empty() {
        rule.push(Span::raw(format!(" · search \"{}\"", app.query.value())));
    }
}

fn filter_strip(rule: &mut Strip, app: &App) {
    rule.push(Span::raw("log  "));
    for (i, filter) in Filter::ALL.into_iter().enumerate() {
        if i > 0 {
            rule.push(Span::raw(" · "));
        }
        let label = Span::raw(filter.label());
        let label = if filter == app.filter {
            label.fg(ACCENT).bold()
        } else {
            label
        };
        rule.target(vec![label], Hit::Filter(filter));
    }
}

fn tab_strip(rule: &mut Strip, app: &App, g: &Game, width: usize) {
    let full = |counts: bool| -> Vec<Field> {
        View::ALL
            .into_iter()
            .map(|view| {
                let mut tab = vec![Span::raw(format!(
                    "{} ",
                    app.keymap.key(Command::View(view))
                ))];
                match view {
                    View::Log => tab.push(Span::raw(format!("log·{}", app.filter.label()))),
                    View::List(section) => {
                        tab.push(Span::raw(section.label().to_lowercase()));
                        if counts {
                            tab.push(Span::raw(" "));
                            tab.extend(section.count(g));
                        }
                    }
                }
                if view == app.view {
                    tab = tab.into_iter().map(|span| span.fg(ACCENT).bold()).collect();
                }
                tab
            })
            .collect()
    };
    let room = |tabs: &[Field]| tabs.iter().map(|t| spans_width(t) + 2).sum::<usize>();
    let counted = full(true);
    let tabs = if rule.width() + room(&counted) <= width {
        counted
    } else {
        full(false)
    };
    for (tab, view) in tabs.into_iter().zip(View::ALL) {
        if view != View::Log {
            rule.push(Span::raw("  "));
        }
        rule.target(tab, Hit::View(view));
    }
}

fn draw_log(f: &mut Frame, area: Rect, app: &App) -> usize {
    let pill_row = usize::from(app.scroll > 0);
    let log = log_lines(
        app,
        usize::from(area.height).saturating_sub(pill_row),
        usize::from(area.width),
    );
    let scrolled = app.scroll.min(app.max_scroll.get());
    let has_log = !log.is_empty();
    let query = app.search(Scope::Log);
    if !has_log && !query.is_empty() {
        let text = Line::raw(format!("  No lines match \"{query}\".")).dark_gray();
        f.render_widget(Paragraph::new(text), area);
        return 0;
    }
    f.render_widget(Paragraph::new(log), area);
    if scrolled > 0 && has_log {
        draw_jump(f, area, app);
    }
    scrolled
}

fn draw_rail(f: &mut Frame, area: Rect, g: &Game, app: &App) {
    let width = usize::from(area.width.saturating_sub(1));
    let sizes = Section::ALL.map(|s| {
        if app.view == View::List(s) {
            0
        } else {
            s.on_rail(g)
        }
    });
    let (gaps, rows) = rail_rows(sizes, usize::from(area.height));
    let mut lines = Vec::new();
    for (i, section) in Section::ALL.into_iter().enumerate() {
        if gaps && i > 0 {
            lines.push(Line::default());
        }
        let start = lines.len();
        lines.push(fit(heading(g, section, app), width));
        let entries: Vec<_> = section.entries(g).into_iter().take(sizes[i]).collect();
        let cut = entries.len() > rows[i] && rows[i] >= 1;
        let fits = if cut {
            rows[i] - 1
        } else {
            entries.len().min(rows[i])
        };
        let more = entries.len() - fits;
        lines.extend(
            entries
                .into_iter()
                .take(fits)
                .map(|(name, value)| Line::from(entry(3, vec![Span::raw(name)], value, width))),
        );
        if cut {
            lines.push(Line::raw(format!("   +{more}")).dark_gray());
        }
        let top = area.y + start as u16;
        let bottom = (area.y + lines.len() as u16).min(area.bottom());
        if top < bottom {
            hit(
                app,
                Rect {
                    y: top,
                    height: bottom - top,
                    ..area
                },
                Hit::View(View::List(section)),
            );
        }
    }
    let rule = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().dark_gray());
    f.render_widget(Paragraph::new(lines).block(rule), area);
}

fn rail_rows(
    sizes: [usize; Section::ALL.len()],
    height: usize,
) -> (bool, [usize; Section::ALL.len()]) {
    let gaps = Section::ALL.len() - 1;
    let mut left = height.saturating_sub(Section::ALL.len());
    let mut rows = [0; Section::ALL.len()];
    for (i, section) in Section::ALL.into_iter().enumerate() {
        rows[i] = sizes[i].min(section.rail_rows()).min(left);
        left -= rows[i];
    }
    let spaced = left >= gaps;
    if spaced {
        left -= gaps;
    }
    let order = [
        Section::Inventory,
        Section::Quests,
        Section::Factions,
        Section::Players,
        Section::Drops,
        Section::Temp,
    ];
    for i in order.map(|s| s as usize) {
        let more = (sizes[i] - rows[i]).min(left);
        rows[i] += more;
        left -= more;
    }
    (spaced, rows)
}

fn heading(g: &Game, section: Section, app: &App) -> Field {
    let open = app.view == View::List(section);
    let mut spans = vec![
        Span::raw(if open { "▌" } else { " " }).fg(ACCENT),
        Span::raw(format!(
            "{} ",
            app.keymap.key(Command::View(View::List(section)))
        ))
        .dark_gray(),
    ];
    let empty = match section {
        Section::Inventory | Section::Bank => "",
        _ => "none",
    };
    if section.len(g) == 0 && !empty.is_empty() {
        spans.push(Span::raw(format!("{} {empty}", section.label())).dark_gray());
        return spans;
    }
    spans.extend([Span::raw(section.label()).bold(), Span::raw(" ")]);
    spans.extend(section.count(g));
    if section == Section::Bank && g.bank.is_none() {
        spans.push(Span::raw("  not opened").dark_gray());
    }
    spans
}

fn tally(used: u32, slots: u32) -> Span<'static> {
    if slots == 0 {
        return Span::raw(used.to_string());
    }
    let text = Span::raw(format!("{used}/{slots}"));
    match fill(used, slots) {
        Some((color, _)) => text.fg(color),
        None => text,
    }
}

fn fill(used: u32, slots: u32) -> Option<(Color, &'static str)> {
    match () {
        _ if slots == 0 => None,
        _ if used >= slots => Some((Color::Red, "full")),
        _ if used * 10 >= slots * 9 => Some((Color::Yellow, "nearly full")),
        _ => None,
    }
}

fn turn_in(q: &QuestProgress) -> Field {
    match () {
        _ if q.need == 0 => vec![Span::raw("ready").green()],
        _ if q.have >= q.need => vec![Span::raw(format!("{}/{} ✓", q.have, q.need)).green()],
        _ => vec![Span::raw(format!("{}/{}", q.have, q.need))],
    }
}

fn standing(f: &Standing) -> Field {
    let rank = Span::raw(format!("rank {}", f.rank));
    match f.next {
        None => vec![rank.green()],
        Some(next) => vec![rank.dark_gray(), Span::raw(format!(" {}/{next}", f.rep))],
    }
}

fn held(s: &Stack) -> Field {
    match s.rank {
        _ if s.equipped => vec![Span::raw("equipped").dark_gray()],
        Some(rank) => vec![Span::raw(format!("rank {rank}"))],
        None if s.max > 1 && s.qty >= s.max => {
            vec![Span::raw(format!("{}/{} ✓", s.qty, s.max)).green()]
        }
        None if s.max > 1 => vec![Span::raw(format!("{}/{}", s.qty, s.max))],
        None if s.qty > 1 => vec![Span::raw(s.qty.to_string())],
        None => Vec::new(),
    }
}

fn entry(indent: usize, name: Field, value: Field, width: usize) -> Field {
    let value_width = spans_width(&value);
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    spans.extend(clip(name, width.saturating_sub(indent + value_width + 1)));
    spans.push(Span::raw(
        " ".repeat(width.saturating_sub(spans_width(&spans) + value_width)),
    ));
    spans.extend(value);
    clip(spans, width)
}

fn draw_section(f: &mut Frame, area: Rect, g: &Game, section: Section, app: &App) {
    let query = app.search(Scope::List(section));
    let all = section.entries(g);
    let none = all.is_empty();
    let entries: Vec<(usize, (String, Field))> = all
        .into_iter()
        .enumerate()
        .filter(|(_, (name, _))| matches(name, query))
        .collect();
    app.list_len.set(entries.len());
    if entries.is_empty() {
        app.list_max.set(0);
        let text = if none {
            section.nothing(g).to_owned()
        } else {
            format!("Nothing matches \"{query}\".")
        };
        let text = Line::raw(format!("  {text}")).dark_gray();
        return f.render_widget(Paragraph::new(text), area);
    }
    let width = usize::from(area.width);
    let widest = entries
        .iter()
        .map(|(_, (name, value))| Span::raw(name.as_str()).width() + spans_width(value))
        .max()
        .unwrap_or(0);
    let picker = section == Section::Drops;
    let cols = if picker {
        1
    } else {
        (width / (widest + 4).clamp(COLUMN_MIN, COLUMN_MAX)).max(1)
    };
    let column = if picker {
        COLUMN_MAX.min(width)
    } else {
        width / cols
    };
    let rows = entries.len().div_ceil(cols);
    let height = usize::from(area.height);
    app.list_max.set(rows.saturating_sub(height));
    let cursor = picker.then(|| app.cursor.min(entries.len() - 1));
    let mut at = app.list_at.get().min(app.list_max.get());
    if let Some(row) = cursor.map(|i| i % rows) {
        at = at.min(row).max((row + 1).saturating_sub(height));
    }
    app.list_at.set(at);
    let marks = |i: usize, original: usize| -> Field {
        let Some(cursor) = cursor else {
            return vec![Span::raw("  ")];
        };
        let picked = g
            .drops
            .get(original)
            .is_some_and(|d| app.picked.contains(&d.id));
        vec![
            Span::raw(if i == cursor { "▌" } else { " " }).fg(ACCENT),
            Span::raw(if picked { "◆" } else { " " }).fg(ACCENT),
        ]
    };
    let lines: Vec<Line> = (at..rows.min(at + height))
        .map(|row| {
            let cells = (0..cols)
                .map(|col| col * rows + row)
                .filter_map(|i| Some((i, entries.get(i)?)));
            let spans = cells.flat_map(|(i, (original, (name, value)))| {
                marks(i, *original)
                    .into_iter()
                    .chain(entry(
                        0,
                        lit(name, 0, &found(name, query)),
                        value.clone(),
                        column.saturating_sub(4),
                    ))
                    .chain([Span::raw("  ")])
            });
            Line::from(spans.collect::<Field>())
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_jump(f: &mut Frame, area: Rect, app: &App) {
    let unseen = app.unseen_from.map_or(0, |from| {
        app.log.since(from).filter(|l| app.shows(l)).count()
    });
    let end = app.keymap.key(Command::Newest);
    let label = if unseen > 0 {
        format!(" {unseen} new · jump to bottom ({end}) ↓ ")
    } else {
        format!(" jump to bottom ({end}) ↓ ")
    };
    let width = (label.chars().count() as u16).min(area.width);
    let pill = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.bottom() - 1,
        width,
        height: 1,
    };
    hit(app, pill, Hit::Jump);
    let style = Style::new().fg(Color::White).bg(Color::DarkGray);
    f.render_widget(Paragraph::new(label).style(style), pill);
}

fn detail(account: &Shown, full: bool, frame: usize, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![identity(account, frame, width)];
    lines.extend(trouble(account).map(|t| fit(vec![t], width)));
    lines.push(fit(task(account), width));
    if let Some(g) = &account.game {
        lines.push(Line::default());
        if full {
            lines.extend(flow(everything(account, g, width), width));
        } else {
            lines.push(fit(summary(account, g).join(&Span::raw("  ")), width));
        }
    }
    lines
}

fn identity(s: &Shown, frame: usize, width: usize) -> Line<'static> {
    let mut left = vec![Span::raw(s.name.clone()).bold()];
    if !s.user.eq_ignore_ascii_case(&s.name) {
        left.push(Span::raw(format!(" ({})", s.user)));
    }
    if !s.server.is_empty() {
        left.push(Span::raw(format!(" · {}", s.server)));
    }
    if let Some(g) = &s.game {
        left.push(Span::raw(format!(" · {} {}/{}", g.area, g.cell, g.pad)));
    }
    let mut right = Status::of(s).badge(frame);
    if let Some(up) = s.up {
        right.push(Span::raw(format!("  up {}", span(up))).dark_gray());
    }
    let left = clip(left, width.saturating_sub(spans_width(&right) + 2));
    spread(left, right, width)
}

fn summary(s: &Shown, g: &Game) -> Vec<Field> {
    let mut fields = vec![hp(g), amount(s.stats.kills, "kill")];
    fields.extend(s.gold_gained.map(gained));
    fields.extend(
        g.target
            .as_ref()
            .map(|t| labeled("Target", vec![Span::raw(t.name.clone())])),
    );
    fields
}

fn everything(s: &Shown, g: &Game, width: usize) -> Vec<Vec<Field>> {
    let class = match g.class.as_str() {
        "" => vec![Span::raw("no class").dark_gray()],
        name => vec![
            Span::raw(format!("{name} ")),
            Span::raw("rank ").dark_gray(),
            Span::raw(g.class_rank.to_string()),
        ],
    };
    let mut you = vec![
        hp(g),
        vitals("MP", g.mp, g.max_mp, Color::Blue),
        labeled("Lv", vec![Span::raw(g.level.to_string())]),
        class,
        vec![
            Span::raw(short(g.gold as i64)),
            Span::raw(" gold").dark_gray(),
        ],
    ];
    if g.away {
        you.push(vec![Span::raw("away from keyboard").yellow()]);
    }
    let target = match &g.target {
        Some(t) => {
            let mut field = labeled("Target", vec![Span::raw(format!("{} ", t.name))]);
            field.extend(meter(t.hp, t.max_hp, 8, Color::Red));
            field.push(Span::raw(format!(" {}/{}", t.hp, t.max_hp)));
            field
        }
        None => vec![Span::raw("no target").dark_gray()],
    };
    let mut combat = Vec::new();
    if !g.skills.is_empty() {
        combat.push(skills(g));
    }
    if !g.auras.is_empty() {
        let auras = g.auras.iter().map(|a| {
            (
                a.name.clone(),
                (a.stacks > 1).then(|| format!("x{}", a.stacks)),
            )
        });
        combat.push(listed("Auras", auras, width));
    }
    let mut stats = vec![
        labeled("Stats", amount(s.stats.kills, "kill")),
        amount(s.stats.drops, "drop"),
    ];
    stats.extend(s.gold_gained.map(gained));
    stats.push(amount(s.stats.deaths, "death"));
    vec![you, vec![target], combat, stats]
}

fn hp(g: &Game) -> Field {
    let color = match () {
        _ if g.dead => Color::DarkGray,
        _ if g.max_hp > 0 && g.hp * 4 < g.max_hp => Color::Red,
        _ => Color::Green,
    };
    vitals("HP", g.hp, g.max_hp, color)
}

fn vitals(label: &str, current: u32, max: u32, color: Color) -> Field {
    let mut field = vec![Span::raw(format!("{label} ")).dark_gray()];
    field.extend(meter(current, max, 8, color));
    let digits = max.to_string().len();
    field.push(Span::raw(format!(" {current:>digits$}/{max}")));
    field
}

fn meter(current: u32, max: u32, width: usize, color: Color) -> Field {
    let filled = if max == 0 {
        0
    } else {
        (u64::from(current.min(max)) * width as u64 / u64::from(max)) as usize
    };
    vec![
        Span::raw("█".repeat(filled)).fg(color),
        Span::raw("░".repeat(width - filled)).dark_gray(),
    ]
}

fn labeled(label: &str, mut value: Field) -> Field {
    value.insert(0, Span::raw(format!("{label} ")).dark_gray());
    value
}

fn amount(n: u64, noun: &str) -> Field {
    let noun = if n == 1 {
        noun.to_owned()
    } else {
        format!("{noun}s")
    };
    vec![
        Span::raw(grouped(n)),
        Span::raw(format!(" {noun}")).dark_gray(),
    ]
}

fn gained(gold: i64) -> Field {
    let text = if gold >= 0 {
        format!("+{}", short(gold))
    } else {
        short(gold)
    };
    vec![Span::raw(text), Span::raw(" gold").dark_gray()]
}

fn skills(g: &Game) -> Field {
    let mut field = vec![Span::raw("Skills ").dark_gray()];
    for (i, s) in g.skills.iter().enumerate() {
        if i > 0 {
            field.push(Span::raw("  "));
        }
        let label = format!("{} {}", i + 1, s.name);
        let lit = (label.chars().count() as f32 * s.ready) as usize;
        let at = label
            .char_indices()
            .nth(lit)
            .map_or(label.len(), |(at, _)| at);
        let (ready, cooling) = label.split_at(at);
        field.push(Span::raw(ready.to_owned()));
        field.push(Span::raw(cooling.to_owned()).dark_gray());
    }
    field
}

fn listed(
    label: &str,
    items: impl ExactSizeIterator<Item = (String, Option<String>)>,
    width: usize,
) -> Field {
    let total = items.len();
    let mut field = vec![Span::raw(format!("{label} ")).dark_gray()];
    for (i, (name, qualifier)) in items.enumerate() {
        let gap = if i == 0 { "" } else { "  " };
        let mut piece = vec![Span::raw(format!("{gap}{name}"))];
        piece.extend(qualifier.map(|q| Span::raw(format!(" {q}")).dark_gray()));
        let rest = total - i - 1;
        let more = if rest == 0 {
            0
        } else {
            format!("  +{rest}").len()
        };
        if spans_width(&field) + spans_width(&piece) + more > width {
            field.push(Span::raw(format!("{gap}+{}", total - i)).dark_gray());
            break;
        }
        field.extend(piece);
    }
    field
}

fn flow(groups: Vec<Vec<Field>>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for group in groups.into_iter().filter(|g| !g.is_empty()) {
        let mut line: Field = Vec::new();
        for field in group {
            if !line.is_empty() && spans_width(&line) + 2 + spans_width(&field) > width {
                lines.push(fit(std::mem::take(&mut line), width));
            }
            if !line.is_empty() {
                line.push(Span::raw("  "));
            }
            line.extend(field);
        }
        lines.push(fit(line, width));
    }
    lines
}

fn fit(spans: Field, width: usize) -> Line<'static> {
    Line::from(clip(spans, width))
}

fn clip(spans: Field, width: usize) -> Field {
    if spans_width(&spans) <= width {
        return spans;
    }
    let mut room = width.saturating_sub(1);
    let mut cut = Vec::new();
    for span in spans {
        let mut text = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if w > room {
                room = 0;
                break;
            }
            room -= w;
            text.push(c);
        }
        cut.push(Span::styled(text, span.style));
        if room == 0 {
            break;
        }
    }
    cut.push(Span::raw("…"));
    cut
}

fn spread(mut left: Field, right: Field, width: usize) -> Line<'static> {
    let gap = width
        .saturating_sub(spans_width(&left) + spans_width(&right))
        .max(2);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    Line::from(left)
}

fn hints<K: AsRef<str>>(keys: &[(K, &str)]) -> Field {
    let mut spans = Vec::new();
    for (i, (key, word)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(key.as_ref().to_owned()).bold());
        spans.push(Span::raw(format!(" {word}")).dark_gray());
    }
    spans
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn log_lines(app: &App, height: usize, width: usize) -> Vec<Line<'static>> {
    app.log_width.set(width);
    let tagged = app.filter.tagged();
    let lines: Vec<&LogLine> = app.log.lines().filter(|l| app.shows(l)).collect();
    let sizes: Vec<usize> = lines
        .iter()
        .map(|l| pieces(l, width, tagged).1.len())
        .collect();
    let total: usize = sizes.iter().sum();
    app.max_scroll.set(total.saturating_sub(height));
    let end = total - app.scroll.min(app.max_scroll.get());
    let start = end.saturating_sub(height);
    let mut rows = Vec::new();
    let mut at = 0;
    for (line, size) in lines.into_iter().zip(sizes) {
        let (first, last) = (at.max(start), (at + size).min(end));
        if first < last {
            rows.extend(
                log_rows(line, width, tagged, app.search(Scope::Log))
                    .into_iter()
                    .skip(first - at)
                    .take(last - first),
            );
        }
        at += size;
        if at >= end {
            break;
        }
    }
    rows
}

fn pieces(l: &LogLine, width: usize, tagged: bool) -> (Field, Vec<&str>) {
    let mut head = vec![
        Span::raw(crate::view::time_of_day(l.at_ms)).dark_gray(),
        Span::raw("  "),
    ];
    if tagged {
        let color = match l.channel {
            Channel::Mira => Color::Cyan,
            Channel::Script => Color::Magenta,
            Channel::Server => Color::Yellow,
            Channel::Chat => Color::Green,
            Channel::Game => Color::Blue,
        };
        head.extend([
            Span::styled(format!("{:<6}", l.channel.label()), Style::new().fg(color)),
            Span::raw("  "),
        ]);
    }
    let room = width.saturating_sub(spans_width(&head)).max(10);
    let mut pieces: Vec<&str> = l.text.lines().flat_map(|line| wrap(line, room)).collect();
    if pieces.is_empty() {
        pieces.push("");
    }
    (head, pieces)
}

pub(super) fn log_rows(l: &LogLine, width: usize, tagged: bool, query: &str) -> Vec<Line<'static>> {
    let (mut head, pieces) = pieces(l, width, tagged);
    let indent = " ".repeat(spans_width(&head));
    let found = found(&l.text, query);
    let text = |piece: &str| match found.as_slice() {
        [] => vec![Span::raw(piece.to_owned())],
        found => lit(
            piece,
            piece.as_ptr() as usize - l.text.as_ptr() as usize,
            found,
        ),
    };
    let mut pieces = pieces.into_iter();
    head.extend(text(pieces.next().unwrap_or_default()));
    let mut rows = vec![Line::from(head)];
    rows.extend(
        pieces.map(|piece| Line::from([vec![Span::raw(indent.clone())], text(piece)].concat())),
    );
    rows
}

fn lit(piece: &str, from: usize, found: &[Range<usize>]) -> Field {
    let mut spans = Vec::new();
    let mut at = 0;
    for range in found {
        let start = range.start.max(from + at) - from;
        let end = range.end.min(from + piece.len()).saturating_sub(from);
        if start < end {
            if at < start {
                spans.push(Span::raw(piece[at..start].to_owned()));
            }
            spans.push(Span::styled(piece[start..end].to_owned(), FOUND));
            at = end;
        }
    }
    if at < piece.len() || spans.is_empty() {
        spans.push(Span::raw(piece[at..].to_owned()));
    }
    spans
}

fn wrap(text: &str, room: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    loop {
        let mut used = 0;
        let cut = rest.char_indices().find_map(|(at, c)| {
            used += c.width().unwrap_or(0);
            (used > room).then_some(at)
        });
        let Some(cut) = cut.filter(|&cut| cut > 0) else {
            pieces.push(rest);
            return pieces;
        };
        let at = rest[..cut].rfind(' ').filter(|&at| at > 0).unwrap_or(cut);
        pieces.push(&rest[..at]);
        rest = rest[at..].trim_start();
        if rest.is_empty() {
            return pieces;
        }
    }
}

fn draw_keys(f: &mut Frame, area: Rect, app: &App) {
    f.buffer_mut()
        .set_style(area, Style::new().fg(Color::DarkGray));
    let groups: Vec<(&str, Vec<(String, &str)>)> = KEY_GROUPS
        .iter()
        .map(|&(title, rows)| {
            let rows = rows
                .iter()
                .map(|&(commands, label)| (app.keymap.keys(commands), label))
                .filter(|(keys, _)| !keys.is_empty())
                .collect();
            (title, rows)
        })
        .collect();
    let rows = || groups.iter().flat_map(|(_, rows)| rows);
    let key_width = rows().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let column = rows()
        .map(|(_, label)| key_width + 2 + label.chars().count())
        .chain(groups.iter().map(|(title, _)| title.chars().count()))
        .max()
        .unwrap_or(0);
    let block = |groups: &[(&str, Vec<(String, &str)>)]| -> Vec<Field> {
        let mut lines = Vec::new();
        for (title, rows) in groups {
            if !lines.is_empty() {
                lines.push(vec![Span::raw(" ".repeat(column))]);
            }
            lines.push(vec![Span::raw(format!("{title:<column$}")).bold()]);
            for (keys, label) in rows {
                lines.push(vec![
                    Span::raw(format!("{keys:<key_width$}  ")).bold(),
                    Span::raw(format!("{label:<w$}", w = column - key_width - 2)),
                ]);
            }
        }
        lines
    };
    let two = 2 * column + KEY_GAP + 4 <= usize::from(area.width);
    let (width, mut lines): (usize, Vec<Line>) = if two {
        let half = groups.len() / 2;
        let (left, right) = (block(&groups[..half]), block(&groups[half..]));
        let blank = || vec![Span::raw(" ".repeat(column))];
        let lines = (0..left.len().max(right.len()))
            .map(|i| {
                let mut spans = left.get(i).cloned().unwrap_or_else(blank);
                spans.push(Span::raw(" ".repeat(KEY_GAP)));
                spans.extend(right.get(i).cloned().unwrap_or_default());
                Line::from(spans)
            })
            .collect();
        (2 * column + KEY_GAP, lines)
    } else {
        (column, block(&groups).into_iter().map(Line::from).collect())
    };
    let mut notes =
        vec!["Account keys act on the marked accounts, or the selected one.".to_owned()];
    if let Some(path) = &app.keymap.path {
        notes.push(format!(
            "Change keys in {}.",
            app.mask.hide(&path.display().to_string())
        ));
    }
    lines.push(Line::default());
    for note in &notes {
        lines.extend(
            wrap(note, width)
                .into_iter()
                .map(|l| Line::raw(l.to_owned()).dark_gray()),
        );
    }
    let rect = centered(area, width as u16 + 4, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" keys ")
        .padding(Padding::horizontal(1));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_ending(f: &mut Frame, area: Rect, online: usize) {
    f.buffer_mut()
        .set_style(area, Style::new().fg(Color::DarkGray));
    let who = match online {
        0 => "No account is online.".to_owned(),
        1 => "This logs out the account that is online.".to_owned(),
        n => format!("This logs out the {n} accounts that are online."),
    };
    let keys = [("enter", "end it"), ("q", "detach"), ("esc", "stay")];
    let mut lines = vec![
        Line::raw("End the mira session?").bold(),
        Line::raw(who).dark_gray(),
    ];
    if online > 0 {
        lines.push(Line::raw("Detaching leaves them running.").dark_gray());
    }
    lines.extend([Line::default(), Line::from(hints(&keys))]);
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let rect = centered(area, widest + 4, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" end session ")
        .padding(Padding::horizontal(1));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_skua(f: &mut Frame, area: Rect, app: &App, skua: &Skua, account: Option<&Shown>) {
    let frame = app.frame;
    f.buffer_mut()
        .set_style(area, Style::new().fg(Color::DarkGray));
    let rect = centered(area, 84, area.height.saturating_sub(2).max(8));
    f.render_widget(Clear, rect);
    let name = account.map_or("", |a| a.name.as_str());
    let block = Block::bordered()
        .title(format!(" Skua options · {name} "))
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let width = inner.width as usize;
    let mut tabs = Vec::new();
    for tab in Tab::ALL {
        if !tabs.is_empty() {
            tabs.push(Span::raw("   "));
        }
        tabs.push(if tab == skua.tab {
            Span::raw(tab.label()).bold().fg(ACCENT).underlined()
        } else {
            Span::raw(tab.label()).dark_gray()
        });
    }
    let about = match (skua.tab, skua.pane()) {
        (Tab::CoreBots, _) => format!(
            "CBO_Storage({}).txt, read when CoreBots starts",
            skua.user.to_lowercase()
        ),
        (Tab::Script, Pane::Ready(settings)) => format!(
            "{}.cfg, shared by every account that runs it",
            settings.storage.as_deref().unwrap_or("default")
        ),
        (Tab::Script, _) => skua.script.display().to_string(),
    };
    let mut lines = vec![
        Line::from(tabs),
        fit(vec![Span::raw(app.mask.hide(&about)).dark_gray()], width),
        Line::default(),
    ];
    let height = (inner.height as usize).saturating_sub(lines.len() + 2);
    match skua.pane() {
        Pane::Loading => lines.push(Line::from(vec![
            Span::raw(SPINNER[frame % SPINNER.len()]).yellow(),
            Span::raw(match skua.tab {
                Tab::CoreBots => " loading",
                Tab::Script => " compiling the script",
            }),
        ])),
        Pane::Failed(why) => lines.extend(
            why.lines()
                .take(height)
                .map(|l| fit(vec![Span::raw(app.mask.hide(l)).red()], width)),
        ),
        Pane::Ready(settings) if settings.options.is_empty() => {
            lines.push(Line::raw("This script has no options.").dark_gray());
        }
        Pane::Ready(settings) => {
            let mut rows = Vec::new();
            let mut at = 0;
            for (i, setting) in settings.options.iter().enumerate() {
                if rows.is_empty() || settings.options[i - 1].group != setting.group {
                    rows.push(Line::raw(setting.group.replace('_', " ")).bold());
                }
                let selected = i == skua.cursor();
                if selected {
                    at = rows.len();
                }
                let label = if setting.label.is_empty() {
                    &setting.name
                } else {
                    &setting.label
                };
                let value = match (&skua.editing, &setting.value) {
                    (Some(input), _) if selected => typed_spans(input),
                    (_, None) => vec![Span::raw("default").dark_gray()],
                    (_, Some(value)) => vec![Span::raw(app.mask.hide(value).replace('_', " "))],
                };
                let mut row = spread(vec![Span::raw(format!("  {label}"))], value, width);
                if selected && skua.editing.is_none() {
                    row = row.reversed();
                }
                rows.push(row);
            }
            let start = (at + 1).saturating_sub(height.max(1));
            lines.extend(rows.into_iter().skip(start).take(height));
        }
    }
    let note = if let Some(why) = &skua.error {
        Span::raw(app.mask.hide(why)).red()
    } else if skua.saving.is_some() {
        Span::raw(format!("{} saving", SPINNER[frame % SPINNER.len()])).yellow()
    } else if !account.is_some_and(super::skua::editable) {
        Span::raw(super::skua::BUSY).yellow()
    } else {
        Span::raw(
            skua.selected()
                .map_or_else(String::new, |s| s.description.clone()),
        )
        .dark_gray()
    };
    let [top, _, bottom] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(lines), top);
    f.render_widget(Paragraph::new(fit(vec![note], width)), bottom);
}

const DESCRIPTION_LINES: usize = 3;
const TAG_LINES: usize = 2;

fn cut(text: &str, width: usize, most: usize) -> Vec<String> {
    let pieces = wrap(text, width);
    let mut kept: Vec<String> = pieces.iter().take(most).map(|l| (*l).to_owned()).collect();
    if pieces.len() > most
        && let Some(last) = kept.last_mut()
    {
        *last = wrap(last, width.saturating_sub(1))[0].to_owned() + "…";
    }
    kept
}

fn draw_picker(f: &mut Frame, area: Rect, app: &App, picker: &Picker) {
    f.buffer_mut()
        .set_style(area, Style::new().fg(Color::DarkGray));
    let rect = centered(area, 84, area.height.saturating_sub(2).max(8));
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .title(" Scripts ")
        .padding(Padding::horizontal(1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let width = inner.width as usize;
    let mut tabs = Vec::new();
    for tab in PickerTab::ALL {
        if !tabs.is_empty() {
            tabs.push(Span::raw("   "));
        }
        tabs.push(if tab == picker.tab {
            Span::raw(tab.label()).bold().fg(ACCENT).underlined()
        } else {
            Span::raw(tab.label()).dark_gray()
        });
    }
    let folder = app.mask.hide(&picker.folder().display().to_string());
    let shown = picker.shown();
    let about = match (picker.tab, &picker.mira, &picker.catalog) {
        (PickerTab::Mira, Ok(entries), _) if !entries.is_empty() => {
            format!("{} scripts · {folder}", grouped(entries.len() as u64))
        }
        (PickerTab::Skua, _, Catalog::Ready(library)) => format!(
            "{} scripts · {} · {folder}",
            grouped(library.entries.len() as u64),
            library::short(&library.commit)
        ),
        _ => folder.clone(),
    };
    let search = if picker.searching || !picker.query.value().is_empty() {
        [
            vec![Span::raw("search: ").bold()],
            typed_spans(&picker.query),
        ]
        .concat()
    } else {
        Vec::new()
    };
    let mut lines = vec![
        Line::from(tabs),
        fit(vec![Span::raw(about).dark_gray()], width),
        Line::from(search),
    ];
    let wrapped = |text: &str| -> Vec<String> {
        app.mask
            .hide(text)
            .lines()
            .flat_map(|l| wrap(l, width))
            .map(str::to_owned)
            .collect()
    };
    if let Some(why) = picker
        .failed
        .as_ref()
        .filter(|_| picker.tab == PickerTab::Skua)
    {
        lines.extend(wrapped(why).into_iter().take(6).map(|l| Line::raw(l).red()));
    }
    let height =
        (inner.height as usize).saturating_sub(lines.len() + 1 + DESCRIPTION_LINES + TAG_LINES);
    match (picker.tab, &picker.mira, &picker.catalog) {
        (PickerTab::Mira, Err(why), _) => lines.extend(
            wrapped(why)
                .into_iter()
                .take(height)
                .map(|l| Line::raw(l).red()),
        ),
        (PickerTab::Mira, Ok(entries), _) if entries.is_empty() => lines.extend([
            fit(
                vec![Span::raw(format!("No scripts in {folder} yet."))],
                width,
            ),
            Line::default(),
            fit(
                vec![
                    Span::raw("Put your .ts and .js scripts in that folder to pick them here.")
                        .dark_gray(),
                ],
                width,
            ),
        ]),
        (PickerTab::Skua, _, Catalog::Missing) => lines.extend([
            Line::raw("mira hasn't downloaded the Skua scripts yet."),
            Line::default(),
            fit(
                hints(&[("u", "downloads them from github.com/auqw/Scripts")]),
                width,
            ),
            fit(
                vec![
                    Span::raw("and builds the Skua worker, which needs the .NET SDK.").dark_gray(),
                ],
                width,
            ),
        ]),
        (PickerTab::Skua, _, Catalog::Failed(why)) => lines.extend(
            wrapped(why)
                .into_iter()
                .take(height)
                .map(|l| Line::raw(l).red()),
        ),
        _ if shown.is_empty() => {
            lines.push(Line::raw("No script matches.").dark_gray());
        }
        _ => {
            let start = (picker.cursor + 1).saturating_sub(height.max(1));
            for (i, entry) in shown.iter().enumerate().skip(start).take(height) {
                let folder = entry.path.rsplit_once('/').map_or("", |(dir, _)| dir);
                let mut row = spread(
                    vec![Span::raw(entry.name.clone())],
                    vec![Span::raw(folder.to_owned()).dark_gray()],
                    width,
                );
                if i == picker.cursor {
                    row = row.reversed();
                }
                lines.push(row);
            }
        }
    }
    let note: Vec<Line> = match &app.update {
        Some(update) => vec![
            Line::raw(format!(
                "{} {}",
                SPINNER[app.frame % SPINNER.len()],
                update.stage
            ))
            .yellow(),
        ],
        None => shown.get(picker.cursor).map_or_else(Vec::new, |e| {
            let tags: Vec<String> = e.tags.iter().map(|t| format!("#{t}")).collect();
            let description = cut(&e.description, width, DESCRIPTION_LINES)
                .into_iter()
                .map(|l| Line::raw(l).dark_gray());
            let tags = cut(&tags.join(" "), width, TAG_LINES)
                .into_iter()
                .map(|l| Line::raw(l).fg(ACCENT));
            description.chain(tags).collect()
        }),
    };
    let [top, _, bottom] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length((DESCRIPTION_LINES + TAG_LINES) as u16),
    ])
    .areas(inner);
    f.render_widget(Paragraph::new(lines), top);
    f.render_widget(Paragraph::new(note), bottom);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn short(n: i64) -> String {
    match n.abs() {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0),
        10_000.. => format!("{:.1}K", n as f64 / 1_000.0),
        _ => n.to_string(),
    }
}

fn span(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}
