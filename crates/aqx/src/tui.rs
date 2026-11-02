use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use aqx_core::{Event, Me, Monster, Phase, State, StopHandle, World};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event as Input, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use crate::output::{self, Update};

const LOG_LINES: usize = 1000;
const WINDOW_MS: u64 = 10 * 60 * 1000;
const WIDE: u16 = 90;

#[derive(Clone)]
pub struct View {
    pub phase: Phase,
    pub server: String,
    pub world: World,
}

pub type Slot = Arc<Mutex<Option<Arc<View>>>>;

struct LogLine {
    t: u64,
    source: &'static str,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Filter {
    #[default]
    All,
    Aqx,
    Server,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Aqx,
            Filter::Aqx => Filter::Server,
            Filter::Server => Filter::All,
        }
    }

    fn shows(self, source: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::Aqx => source == "aqx",
            Filter::Server => source == "server",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Aqx => "aqx",
            Filter::Server => "server",
        }
    }
}

#[derive(Default)]
struct App {
    log: VecDeque<LogLine>,
    filter: Filter,
    scroll: usize,
    max_scroll: Cell<usize>,
    kills: VecDeque<u64>,
    drops: VecDeque<u64>,
    deaths: VecDeque<u64>,
    gold: VecDeque<(u64, i64)>,
}

impl App {
    fn push(&mut self, update: Update) {
        let (t, source, text) = match update {
            Update::Say { t, source, text } => (t, source, text),
            Update::Event { t, event } => {
                match event {
                    Event::Killed { .. } => self.kills.push_back(t),
                    Event::DropOffered { .. } => self.drops.push_back(t),
                    Event::Died => self.deaths.push_back(t),
                    _ => {}
                }
                let Some((source, text)) = output::describe(&event) else { return };
                (t, source, text)
            }
        };
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(LogLine { t, source, text });
    }

    fn sample(&mut self, now: u64, view: Option<&View>) {
        if let Some(me) = view.filter(|v| v.phase == Phase::Ready).and_then(|v| v.world.me.as_ref())
            && self.gold.back().is_none_or(|(t, _)| now - t >= 5000)
        {
            self.gold.push_back((now, me.gold));
        }
        let cutoff = now.saturating_sub(WINDOW_MS);
        for times in [&mut self.kills, &mut self.drops, &mut self.deaths] {
            while times.front().is_some_and(|t| *t < cutoff) {
                times.pop_front();
            }
        }
        while self.gold.front().is_some_and(|(t, _)| *t < cutoff) {
            self.gold.pop_front();
        }
    }
}

pub fn run(rx: Receiver<Update>, slot: Slot, stop: StopHandle, bot: JoinHandle<Result<(), String>>) -> Result<(), String> {
    let mut terminal = ratatui::init();
    let mut app = App::default();
    let result = loop {
        while let Ok(update) = rx.try_recv() {
            app.push(update);
        }
        let view = slot.lock().ok().and_then(|v| v.clone());
        let now = output::now();
        app.sample(now, view.as_deref());
        let _ = terminal.draw(|f| draw(f, &app, view.as_deref(), now));
        if bot.is_finished() {
            while let Ok(update) = rx.try_recv() {
                app.push(update);
            }
            break bot.join().unwrap_or_else(|_| Err("the bot thread crashed".into()));
        }
        if !event::poll(Duration::from_millis(250)).unwrap_or(false) {
            continue;
        }
        let Ok(Input::Key(key)) = event::read() else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q' | 'x') => stop.stop(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => stop.stop(),
            KeyCode::Char('l') => {
                app.filter = app.filter.next();
                app.scroll = 0;
            }
            KeyCode::Up => app.scroll = (app.scroll + 1).min(app.max_scroll.get()),
            KeyCode::Down => app.scroll = app.scroll.saturating_sub(1),
            KeyCode::PageUp => app.scroll = (app.scroll + 10).min(app.max_scroll.get()),
            KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(10),
            KeyCode::End => app.scroll = 0,
            _ => {}
        }
    };
    ratatui::restore();
    let start = app.log.len().saturating_sub(10);
    for line in app.log.iter().skip(start) {
        println!("{}  {:<6}  {}", output::clock(line.t), line.source, line.text);
    }
    result
}

fn draw(f: &mut Frame, app: &App, view: Option<&View>, now: u64) {
    let [header, body, footer] = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    draw_header(f, header, view, now);
    f.render_widget(Paragraph::new(" l log · ↑↓ scroll · q quit").dark_gray(), footer);
    let world = view.map(|v| &v.world);
    if body.width >= WIDE {
        let [left, right] = Layout::horizontal([Constraint::Min(40), Constraint::Length(32)]).areas(body);
        let [combat, log] = Layout::vertical([Constraint::Length(7), Constraint::Min(3)]).areas(left);
        let [vitals, rates, area] = Layout::vertical([Constraint::Length(6), Constraint::Length(6), Constraint::Min(3)]).areas(right);
        draw_combat(f, combat, world);
        draw_log(f, log, app);
        draw_vitals(f, vitals, world);
        draw_rates(f, rates, app, now);
        draw_area(f, area, world);
    } else {
        let [vitals, here, log] = Layout::vertical([Constraint::Length(6), Constraint::Length(6), Constraint::Min(3)]).areas(body);
        draw_vitals(f, vitals, world);
        draw_here(f, here, world, app, now);
        draw_log(f, log, app);
    }
}

fn waiting<'a>(title: &'a str) -> Paragraph<'a> {
    Paragraph::new("waiting for the server").dark_gray().block(panel(format!(" {title} ")))
}

fn draw_header(f: &mut Frame, area: Rect, view: Option<&View>, now: u64) {
    let phase = match view.map(|v| &v.phase) {
        None => "CONNECTING".yellow(),
        Some(Phase::Handshaking | Phase::LoggingIn) => "LOGGING IN".yellow(),
        Some(Phase::Loading) => "LOADING".yellow(),
        Some(Phase::Ready) => "READY".green(),
        Some(Phase::Closed(_)) => "CLOSED".red(),
    };
    let mut spans = vec![Span::raw(" ")];
    if let Some(view) = view
        && let Some(me) = &view.world.me
    {
        spans.push(Span::raw(format!("{} · {} · ", me.user, view.server)).bold());
    }
    spans.push(phase.bold());
    spans.push(Span::raw(format!(" · idle · up {}", uptime(now))));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_vitals(f: &mut Frame, area: Rect, world: Option<&World>) {
    let Some(me) = world.and_then(|w| w.me.as_ref()) else { return f.render_widget(waiting("Vitals"), area) };
    let v = me.vitals;
    let hp_color = match v.state {
        State::Dead => Color::DarkGray,
        _ if v.max_hp > 0 && v.hp * 4 < v.max_hp => Color::Red,
        _ => Color::Green,
    };
    let lines = vec![
        Line::from(vec![Span::raw("HP "), Span::styled(bar(v.hp, v.max_hp, 10), Style::new().fg(hp_color)), Span::raw(format!(" {}/{}", v.hp, v.max_hp))]),
        Line::from(vec![Span::raw("MP "), Span::styled(bar(v.mp, v.max_mp, 10), Style::new().fg(Color::Blue)), Span::raw(format!(" {}/{}", v.mp, v.max_mp))]),
        Line::raw(format!("{} · Lv {}", if me.class.is_empty() { "no class" } else { &me.class }, me.level)),
        Line::raw(format!("Gold {}", short(me.gold))),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" Vitals ")), area);
}

fn draw_combat(f: &mut Frame, area: Rect, world: Option<&World>) {
    let Some((me, map)) = world.and_then(|w| Some((w.me.as_ref()?, w.area.as_ref()?))) else {
        return f.render_widget(waiting("Combat"), area);
    };
    let skills = me.skills.iter().skip(1).take(4).enumerate().map(|(i, s)| format!("{} {}", i + 1, s.name)).collect::<Vec<_>>().join("  ");
    let lines = vec![
        Line::raw(format!("Where   {} {}/{}", map.name, me.cell, me.pad)),
        Line::raw(format!("Cell    {}", monsters(map.monsters.iter().filter(|m| m.cell == me.cell)))),
        Line::raw(format!("Map     {}", monsters(map.monsters.iter()))),
        Line::raw(format!("Skills  {}", if skills.is_empty() { "none".into() } else { skills })),
        Line::raw(format!("Auras   {}", auras(me))),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" Combat ")), area);
}

fn draw_here(f: &mut Frame, area: Rect, world: Option<&World>, app: &App, now: u64) {
    let Some((me, map)) = world.and_then(|w| Some((w.me.as_ref()?, w.area.as_ref()?))) else {
        return f.render_widget(waiting("Here"), area);
    };
    let in_cell = map.players.iter().filter(|p| p.cell == me.cell).count() + 1;
    let lines = vec![
        Line::raw(format!("{} {}/{}", map.name, me.cell, me.pad)),
        Line::raw(format!("{} here, {in_cell} in your cell", count(map.players.len() + 1, "player"))),
        Line::raw(format!("Cell {}", monsters(map.monsters.iter().filter(|m| m.cell == me.cell)))),
        Line::raw(format!("kills/h {} · deaths {}", rate(app.kills.len(), now), app.deaths.len())),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" Here ")), area);
}

fn draw_rates(f: &mut Frame, area: Rect, app: &App, now: u64) {
    let gold = match (app.gold.front(), app.gold.back()) {
        (Some((t0, g0)), Some((t1, g1))) if t1 - t0 >= 60_000 => short((g1 - g0) * 3_600_000 / (t1 - t0) as i64),
        _ => "-".into(),
    };
    let row = |label: &str, value: String| Line::raw(format!("{label:<12}{value:>14}"));
    let lines = vec![
        row("kills/h", rate(app.kills.len(), now)),
        row("drops/h", rate(app.drops.len(), now)),
        row("gold/h", gold),
        row("deaths", app.deaths.len().to_string()),
    ];
    f.render_widget(Paragraph::new(lines).block(panel(" Last 10 min ")), area);
}

fn draw_area(f: &mut Frame, area: Rect, world: Option<&World>) {
    let Some((world, me, map)) = world.and_then(|w| Some((w, w.me.as_ref()?, w.area.as_ref()?))) else {
        return f.render_widget(waiting("Area"), area);
    };
    let in_cell: Vec<_> = map.players.iter().filter(|p| p.cell == me.cell).collect();
    let mut lines = vec![Line::raw(format!("{}, {} in your cell", count(map.players.len() + 1, "player"), in_cell.len() + 1))];
    lines.extend(in_cell.iter().map(|p| Line::raw(format!("  {} Lv {}", p.name, p.level)).dark_gray()));
    if !world.drops.is_empty() {
        lines.push(Line::raw("Drops on offer"));
        lines.extend(world.drops.iter().map(|d| Line::raw(format!("  {} x{}", d.name, d.qty))));
    }
    f.render_widget(Paragraph::new(lines).block(panel(" Area ")), area);
}

fn monsters<'a>(list: impl Iterator<Item = &'a Monster>) -> String {
    let (total, alive) = list.fold((0, 0), |(t, a), m| (t + 1, a + usize::from(m.vitals.state != State::Dead)));
    format!("{}, {alive} alive", count(total, "monster"))
}

fn auras(me: &Me) -> String {
    let names: Vec<_> = me.auras.iter().map(|a| if a.stacks > 1 { format!("{} x{}", a.name, a.stacks) } else { a.name.clone() }).collect();
    if names.is_empty() { "none".into() } else { names.join(", ") }
}

fn rate(n: usize, now: u64) -> String {
    let window = now.min(WINDOW_MS);
    if window < 60_000 { "-".into() } else { short(n as i64 * 3_600_000 / window as i64) }
}

fn draw_log(f: &mut Frame, area: Rect, app: &App) {
    let shown: Vec<&LogLine> = app.log.iter().filter(|l| app.filter.shows(l.source)).collect();
    let height = area.height.saturating_sub(2) as usize;
    app.max_scroll.set(shown.len().saturating_sub(height));
    let end = shown.len().saturating_sub(app.scroll.min(app.max_scroll.get()));
    let start = end.saturating_sub(height);
    let lines: Vec<Line> = shown[start..end]
        .iter()
        .map(|l| {
            let color = if l.source == "server" { Color::Yellow } else { Color::Cyan };
            Line::from(vec![
                Span::raw(output::clock(l.t)).dark_gray(),
                Span::raw("  "),
                Span::styled(format!("{:<6}", l.source), Style::new().fg(color)),
                Span::raw("  "),
                Span::raw(l.text.as_str()),
            ])
        })
        .collect();
    let title = match output::dropped() {
        0 => format!(" Log · {} ", app.filter.label()),
        n => format!(" Log · {} · {n} lines dropped ", app.filter.label()),
    };
    f.render_widget(Paragraph::new(lines).block(panel(title)), area);
}

fn panel<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered().title(title).padding(Padding::horizontal(1))
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

fn bar(current: u32, max: u32, width: u32) -> String {
    let filled = if max == 0 { 0 } else { (current.min(max) as u64 * width as u64 / max as u64) as usize };
    format!("{}{}", "█".repeat(filled), "░".repeat(width as usize - filled))
}

fn short(n: i64) -> String {
    match n.abs() {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0),
        10_000.. => format!("{:.1}K", n as f64 / 1_000.0),
        _ => n.to_string(),
    }
}

fn uptime(ms: u64) -> String {
    let m = ms / 60_000;
    if m >= 60 { format!("{}h{:02}m", m / 60, m % 60) } else { format!("{m}m") }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use aqx_core::reduce;
    use aqx_protocol::Message;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn screen(app: &App, view: Option<&View>, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|f| draw(f, app, view, 90_000)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer.content.chunks(width as usize).map(|row| row.iter().map(|c| c.symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn shows_the_player_the_map_and_the_log() {
        let mut world = World::default();
        let json = |o: &str| format!(r#"{{"t":"xt","b":{{"r":-1,"o":{o}}}}}"#);
        for text in [
            "%xt%loginResponse%-1%true%1001%hero%Welcome!%".to_string(),
            "<msg t='sys'><body action='joinOK' r='5'><uLs r='5'><u i='1001'><n><![CDATA[hero]]></n></u><u i='1002'><n><![CDATA[ally]]></n></u></uLs></body></msg>".into(),
            json(r#"{"cmd":"moveToArea","areaId":5,"areaName":"farm-1","strMapName":"farm",
                "mondef":[{"MonID":10,"strMonName":"Mosquito"}],
                "monmap":[{"MonMapID":1,"MonID":10,"strFrame":"Enter"},{"MonMapID":2,"MonID":10,"strFrame":"r2"}],
                "monBranch":[{"MonMapID":1,"intHP":540,"intHPMax":540,"intState":1},{"MonMapID":2,"intHP":0,"intHPMax":540,"intState":0}],
                "uoBranch":[{"entID":1001,"uoName":"hero","strFrame":"Enter","strPad":"Spawn","intHP":2000,"intHPMax":4000,"intMP":100,"intMPMax":100,"intState":1},
                            {"entID":1002,"uoName":"ally","intLevel":40,"strFrame":"Enter","strPad":"Left","intHP":900,"intHPMax":1000,"intState":1}]}"#),
            json(r#"{"cmd":"initUserData","uid":1001,"data":{"intLevel":100,"intGold":12300000,"strClassName":"Barber"}}"#),
            json(r#"{"cmd":"ct","a":[{"cmd":"aura+","tInf":"p:1001","auras":[{"nam":"Corruption","dur":20,"stk":2}]}]}"#),
        ] {
            reduce(&mut world, &Message::parse(&text).unwrap(), Instant::now());
        }
        let view = View { phase: Phase::Ready, server: "Twilly".into(), world };
        let mut app = App::default();
        app.push(Update::Say { t: 0, source: "aqx", text: "logging in as hero".into() });
        app.push(Update::Event { t: 30_000, event: Event::Killed { id: aqx_protocol::MonMapId(1), monster: "Mosquito".into() } });
        app.push(Update::Event { t: 31_000, event: Event::ServerText { text: "Profanity filter On.".into() } });

        let shown = screen(&app, Some(&view), 100);
        println!("{shown}");
        for expected in [
            "hero · Twilly · READY · idle · up 1m",
            "Where   farm-1 Enter/Spawn",
            "HP █████░░░░░ 2000/4000",
            "Barber · Lv 100",
            "Gold 12.3M",
            "Cell    1 monster, 1 alive",
            "Map     2 monsters, 1 alive",
            "Auras   Corruption x2",
            "2 players, 2 in your cell",
            "kills/h",
            "0:00:30.000  aqx     killed Mosquito",
            "0:00:31.000  server  Profanity filter On.",
        ] {
            assert!(shown.contains(expected), "missing {expected:?}");
        }

        let narrow = screen(&app, Some(&view), 50);
        println!("{narrow}");
        for expected in ["hero · Twilly · READY", "HP █████░░░░░ 2000/4000", "farm-1 Enter/Spawn", "2 players here, 2 in your cell", "killed Mosquito", "q quit"] {
            assert!(narrow.contains(expected), "narrow screen is missing {expected:?}");
        }

        app.filter = Filter::Server;
        let shown = screen(&app, Some(&view), 100);
        assert!(shown.contains("Profanity filter On.") && !shown.contains("killed Mosquito"));
    }

    #[test]
    fn scrolling_past_the_oldest_line_keeps_the_log_on_screen() {
        let mut app = App::default();
        for i in 0..5 {
            app.push(Update::Say { t: i, source: "aqx", text: format!("line {i}") });
        }
        app.scroll = 500;
        let shown = screen(&app, None, 100);
        assert!(shown.contains("line 0") && shown.contains("line 4"));
    }

    #[test]
    fn gold_is_sampled_only_once_the_session_is_ready() {
        let mut world = World::default();
        reduce(&mut world, &Message::parse("%xt%loginResponse%-1%true%1001%hero%Welcome!%").unwrap(), Instant::now());
        let mut app = App::default();
        app.sample(1_000, Some(&View { phase: Phase::Loading, server: "Twilly".into(), world: world.clone() }));
        assert!(app.gold.is_empty());
        app.sample(2_000, Some(&View { phase: Phase::Ready, server: "Twilly".into(), world }));
        assert_eq!(app.gold.len(), 1);
    }

    #[test]
    fn draws_every_panel_before_the_server_answers() {
        let shown = screen(&App::default(), None, 100);
        assert!(shown.contains("CONNECTING"));
        for title in ["Combat", "Vitals", "Last 10 min", "Area", "Log"] {
            assert!(shown.contains(title), "missing panel {title}");
        }
        assert_eq!(shown.matches("waiting for the server").count(), 3);
    }
}
