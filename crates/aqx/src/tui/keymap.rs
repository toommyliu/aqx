use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;

use super::{Section, View};

pub const FILE_NAME: &str = "keys.toml";
pub const SCHEMA_NAME: &str = "keys.schema.json";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Command {
    Focus,
    Up,
    Down,
    Left,
    Right,
    Mark,
    MarkAll,
    Open,
    Search,
    Back,
    LogIn,
    LogOut,
    PickScript,
    TypeScript,
    Stop,
    Restart,
    Pause,
    SwitchServer,
    View(View),
    PreviousChannel,
    NextChannel,
    Dashboard,
    Details,
    Zoom,
    NarrowerSidebar,
    WiderSidebar,
    PageUp,
    PageDown,
    Oldest,
    Newest,
    Help,
    Detach,
    EndSession,
}

pub(super) struct Entry {
    pub command: Command,
    pub name: &'static str,
    pub keys: &'static [&'static str],
    pub about: &'static str,
}

const fn entry(
    command: Command,
    name: &'static str,
    keys: &'static [&'static str],
    about: &'static str,
) -> Entry {
    Entry {
        command,
        name,
        keys,
        about,
    }
}

pub(super) const COMMANDS: &[Entry] = &[
    entry(
        Command::Focus,
        "focus",
        &["tab"],
        "Switch between the account list and the main pane.",
    ),
    entry(Command::Up, "up", &["up"], "Move up in the focused pane."),
    entry(
        Command::Down,
        "down",
        &["down"],
        "Move down in the focused pane.",
    ),
    entry(
        Command::Left,
        "left",
        &["left"],
        "Move left on the dashboard.",
    ),
    entry(
        Command::Right,
        "right",
        &["right"],
        "Move right on the dashboard.",
    ),
    entry(
        Command::Mark,
        "mark",
        &["space"],
        "Mark the account or drop under the cursor.",
    ),
    entry(
        Command::MarkAll,
        "mark_all",
        &["a"],
        "Mark every account or drop, or unmark them all.",
    ),
    entry(
        Command::Open,
        "open",
        &["enter"],
        "Open the selected account, or accept the marked drops.",
    ),
    entry(
        Command::Search,
        "search",
        &["/"],
        "Search the focused pane.",
    ),
    entry(
        Command::Back,
        "back",
        &["esc"],
        "Clear the search, go back to the log, then unmark.",
    ),
    entry(Command::LogIn, "log_in", &["i"], "Log in."),
    entry(Command::LogOut, "log_out", &["x"], "Log out."),
    entry(
        Command::PickScript,
        "pick_script",
        &["o"],
        "Pick a script to run.",
    ),
    entry(
        Command::TypeScript,
        "type_script",
        &["O"],
        "Type a script path and args to run.",
    ),
    entry(Command::Stop, "stop", &["s"], "Stop the script."),
    entry(Command::Restart, "restart", &["r"], "Restart the script."),
    entry(
        Command::Pause,
        "pause",
        &["p"],
        "Pause the script, or resume it.",
    ),
    entry(
        Command::SwitchServer,
        "switch_server",
        &["c"],
        "Log in to another server, then run the script again.",
    ),
    entry(
        Command::View(View::Log),
        "view_log",
        &["1"],
        "Show the log.",
    ),
    entry(
        Command::View(View::List(Section::Players)),
        "view_players",
        &["2"],
        "Show the players in the cell.",
    ),
    entry(
        Command::View(View::List(Section::Drops)),
        "view_drops",
        &["3"],
        "Show the drops waiting to be accepted.",
    ),
    entry(
        Command::View(View::List(Section::Quests)),
        "view_quests",
        &["4"],
        "Show the accepted quests.",
    ),
    entry(
        Command::View(View::List(Section::Temp)),
        "view_temp",
        &["5"],
        "Show the temporary items.",
    ),
    entry(
        Command::View(View::List(Section::Inventory)),
        "view_inventory",
        &["6"],
        "Show the inventory.",
    ),
    entry(
        Command::View(View::List(Section::Bank)),
        "view_bank",
        &["7"],
        "Show the bank.",
    ),
    entry(
        Command::View(View::List(Section::Factions)),
        "view_factions",
        &["8"],
        "Show the faction ranks.",
    ),
    entry(
        Command::PreviousChannel,
        "previous_channel",
        &["["],
        "Show the log channel to the left.",
    ),
    entry(
        Command::NextChannel,
        "next_channel",
        &["]"],
        "Show the log channel to the right.",
    ),
    entry(
        Command::Dashboard,
        "dashboard",
        &["g"],
        "Show every account as a card.",
    ),
    entry(
        Command::Details,
        "details",
        &["d"],
        "Show more of the selected account.",
    ),
    entry(
        Command::Zoom,
        "zoom",
        &["z"],
        "Hide the sidebar and the side panel, or show them.",
    ),
    entry(
        Command::NarrowerSidebar,
        "narrower_sidebar",
        &["<"],
        "Make the sidebar narrower.",
    ),
    entry(
        Command::WiderSidebar,
        "wider_sidebar",
        &[">"],
        "Make the sidebar wider.",
    ),
    entry(
        Command::PageUp,
        "page_up",
        &["pageup"],
        "Scroll the main pane up a page.",
    ),
    entry(
        Command::PageDown,
        "page_down",
        &["pagedown"],
        "Scroll the main pane down a page.",
    ),
    entry(
        Command::Oldest,
        "oldest",
        &["home"],
        "Scroll the main pane to the top.",
    ),
    entry(
        Command::Newest,
        "newest",
        &["end"],
        "Scroll the main pane to the newest lines.",
    ),
    entry(Command::Help, "help", &["?"], "List every key."),
    entry(
        Command::Detach,
        "detach",
        &["q"],
        "Close the TUI and leave the accounts running.",
    ),
    entry(
        Command::EndSession,
        "end_session",
        &["Q"],
        "Ask to log every account out and end the session.",
    ),
];

impl Command {
    fn at(self) -> usize {
        COMMANDS
            .iter()
            .position(|e| e.command == self)
            .expect("every command is listed")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Key {
    code: KeyCode,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

impl Key {
    fn of(event: KeyEvent) -> Key {
        let held = |m| event.modifiers.contains(m);
        Key {
            code: event.code,
            ctrl: held(KeyModifiers::CONTROL),
            alt: held(KeyModifiers::ALT),
            shift: held(KeyModifiers::SHIFT)
                && !matches!(event.code, KeyCode::Char(_) | KeyCode::BackTab),
        }
    }

    fn parse(text: &str) -> Result<Key, String> {
        let (held, name) = match text.strip_suffix("++") {
            Some(held) => (held, "+"),
            None if text.chars().count() == 1 => ("", text),
            None => text.rsplit_once('+').unwrap_or(("", text)),
        };
        let mut key = Key {
            code: KeyCode::Null,
            ctrl: false,
            alt: false,
            shift: false,
        };
        for modifier in held.split('+').filter(|m| !m.is_empty()) {
            match modifier.to_lowercase().as_str() {
                "ctrl" => key.ctrl = true,
                "alt" => key.alt = true,
                "shift" => key.shift = true,
                _ => return Err(format!("{text:?} has an unknown modifier {modifier:?}")),
            }
        }
        let mut chars = name.chars();
        key.code = match (chars.next(), chars.next()) {
            (Some(c), None) if key.shift && c.is_alphabetic() => {
                key.shift = false;
                KeyCode::Char(c.to_uppercase().next().unwrap_or(c))
            }
            (Some(_), None) if key.shift => {
                return Err(format!(
                    "{text:?} needs the character shift types, like \"!\" for shift+1"
                ));
            }
            (Some(c), None) => KeyCode::Char(c),
            _ => match name.to_lowercase().as_str() {
                "up" => KeyCode::Up,
                "down" => KeyCode::Down,
                "left" => KeyCode::Left,
                "right" => KeyCode::Right,
                "enter" => KeyCode::Enter,
                "esc" => KeyCode::Esc,
                "tab" if key.shift => {
                    key.shift = false;
                    KeyCode::BackTab
                }
                "tab" => KeyCode::Tab,
                "space" => KeyCode::Char(' '),
                "backspace" => KeyCode::Backspace,
                "delete" => KeyCode::Delete,
                "insert" => KeyCode::Insert,
                "home" => KeyCode::Home,
                "end" => KeyCode::End,
                "pageup" => KeyCode::PageUp,
                "pagedown" => KeyCode::PageDown,
                f => match f.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    Some(n @ 1..=12) => KeyCode::F(n),
                    _ => return Err(format!("{text:?} isn't a key aqx knows")),
                },
            },
        };
        Ok(key)
    }

    fn name(self) -> Option<String> {
        let name = match self.code {
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Up => "up".to_owned(),
            KeyCode::Down => "down".to_owned(),
            KeyCode::Left => "left".to_owned(),
            KeyCode::Right => "right".to_owned(),
            KeyCode::Enter => "enter".to_owned(),
            KeyCode::Esc => "esc".to_owned(),
            KeyCode::Tab | KeyCode::BackTab => "tab".to_owned(),
            KeyCode::Backspace => "backspace".to_owned(),
            KeyCode::Delete => "delete".to_owned(),
            KeyCode::Insert => "insert".to_owned(),
            KeyCode::Home => "home".to_owned(),
            KeyCode::End => "end".to_owned(),
            KeyCode::PageUp => "pageup".to_owned(),
            KeyCode::PageDown => "pagedown".to_owned(),
            KeyCode::F(n @ 1..=12) => format!("f{n}"),
            _ => return None,
        };
        let shift = self.shift || self.code == KeyCode::BackTab;
        let held = [(self.ctrl, "ctrl+"), (self.alt, "alt+"), (shift, "shift+")];
        let prefix: String = held.iter().filter(|(on, _)| *on).map(|(_, m)| *m).collect();
        Some(format!("{prefix}{name}"))
    }

    fn label(self) -> String {
        let name = match self.code {
            KeyCode::Up => "↑".to_owned(),
            KeyCode::Down => "↓".to_owned(),
            KeyCode::Left => "←".to_owned(),
            KeyCode::Right => "→".to_owned(),
            KeyCode::Enter => "enter".to_owned(),
            KeyCode::Esc => "esc".to_owned(),
            KeyCode::Tab => "tab".to_owned(),
            KeyCode::BackTab => "shift+tab".to_owned(),
            KeyCode::Char(' ') => "space".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Backspace => "backspace".to_owned(),
            KeyCode::Delete => "del".to_owned(),
            KeyCode::Insert => "ins".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::PageUp => "PgUp".to_owned(),
            KeyCode::PageDown => "PgDn".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        let held = [
            (self.ctrl, "ctrl+"),
            (self.alt, "alt+"),
            (self.shift, "shift+"),
        ];
        let prefix: String = held.iter().filter(|(on, _)| *on).map(|(_, m)| *m).collect();
        format!("{prefix}{name}")
    }
}

#[derive(Debug)]
pub struct Keymap {
    bound: Vec<Vec<Key>>,
    lookup: HashMap<Key, Command>,
    pub path: Option<PathBuf>,
}

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap::with(HashMap::new()).expect("the default keys don't clash")
    }
}

impl Keymap {
    pub fn load(path: PathBuf) -> Result<Keymap, String> {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut keymap = Keymap::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        keymap.path = Some(path);
        Ok(keymap)
    }

    pub fn parse(text: &str) -> Result<Keymap, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.message().to_owned())?;
        let mut chosen = HashMap::new();
        for (name, value) in table {
            let Some(entry) = COMMANDS.iter().find(|e| e.name == name) else {
                return Err(format!("{name:?} isn't a command"));
            };
            let texts: Vec<&str> = match &value {
                toml::Value::String(key) => vec![key.as_str()],
                toml::Value::Array(keys) => keys
                    .iter()
                    .map(|key| key.as_str().ok_or(()))
                    .collect::<Result<_, _>>()
                    .map_err(|()| format!("{name} takes a key or a list of keys"))?,
                _ => return Err(format!("{name} takes a key or a list of keys")),
            };
            let keys = texts
                .into_iter()
                .map(Key::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("{name}: {e}"))?;
            chosen.insert(entry.command.at(), keys);
        }
        Keymap::with(chosen)
    }

    fn with(mut chosen: HashMap<usize, Vec<Key>>) -> Result<Keymap, String> {
        let bound: Vec<Vec<Key>> = COMMANDS
            .iter()
            .enumerate()
            .map(|(at, e)| {
                chosen.remove(&at).unwrap_or_else(|| {
                    e.keys
                        .iter()
                        .map(|k| Key::parse(k).expect("a default key parses"))
                        .collect()
                })
            })
            .collect();
        let ctrl_c = Key::parse("ctrl+c").expect("ctrl+c parses");
        let mut lookup = HashMap::new();
        for (entry, keys) in COMMANDS.iter().zip(&bound) {
            for &key in keys {
                if key == ctrl_c {
                    return Err(format!(
                        "{}: ctrl+c always asks to end the session",
                        entry.name
                    ));
                }
                if let Some(other) = lookup.insert(key, entry.command) {
                    let other = COMMANDS[other.at()].name;
                    return Err(format!(
                        "{} is bound to both {other} and {}",
                        key.label(),
                        entry.name
                    ));
                }
            }
        }
        Ok(Keymap {
            bound,
            lookup,
            path: None,
        })
    }

    pub(super) fn rebind(&self, command: Command, key: Option<KeyEvent>) -> Result<Keymap, String> {
        let path = self
            .path
            .as_ref()
            .ok_or("aqx has no home folder to keep keys in")?;
        let shown = path.display();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{shown}: {e}")),
        };
        let mut file: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{shown}: {e}"))?;
        let name = COMMANDS[command.at()].name;
        match key {
            Some(event) => {
                let key = Key::of(event).name().ok_or("aqx can't use that key")?;
                let mut value = toml_edit::Value::from(key);
                if let Some(old) = file.get(name).and_then(toml_edit::Item::as_value) {
                    *value.decor_mut() = old.decor().clone();
                }
                file[name] = toml_edit::Item::Value(value);
            }
            None => {
                file.remove(name);
            }
        }
        let text = file.to_string();
        let mut keymap = Keymap::parse(&text)?;
        save(path, &text).map_err(|e| format!("{shown}: {e}"))?;
        keymap.path = Some(path.clone());
        Ok(keymap)
    }

    pub(super) fn is_default(&self, command: Command) -> bool {
        let defaults = COMMANDS[command.at()]
            .keys
            .iter()
            .map(|k| Key::parse(k).ok());
        self.bound[command.at()]
            .iter()
            .copied()
            .map(Some)
            .eq(defaults)
    }

    pub(super) fn command(&self, event: KeyEvent) -> Option<Command> {
        self.lookup.get(&Key::of(event)).copied()
    }

    pub(super) fn key(&self, command: Command) -> String {
        self.bound[command.at()]
            .first()
            .map(|k| k.label())
            .unwrap_or_default()
    }

    pub(super) fn keys(&self, commands: &[Command]) -> String {
        if let [single] = commands {
            let labels: Vec<String> = self.bound[single.at()].iter().map(|k| k.label()).collect();
            return labels.join(" ");
        }
        let firsts: Vec<String> = commands.iter().map(|&c| self.key(c)).collect();
        let digits: Vec<u32> = firsts
            .iter()
            .filter(|k| k.len() == 1)
            .filter_map(|k| k.parse().ok())
            .collect();
        if let [first, .., last] = digits[..]
            && digits.len() == commands.len()
            && digits.len() > 2
            && digits.windows(2).all(|w| w[1] == w[0] + 1)
        {
            return format!("{first}-{last}");
        }
        let firsts: Vec<String> = firsts.into_iter().filter(|k| !k.is_empty()).collect();
        firsts.join(" ")
    }
}

fn save(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let fresh = path.with_extension("toml.new");
    std::fs::write(&fresh, text)?;
    std::fs::rename(&fresh, path)
}

pub fn schema() -> String {
    let properties: serde_json::Map<String, serde_json::Value> = COMMANDS
        .iter()
        .map(|e| {
            let defaults = e.keys.join("\", \"");
            let value = json!({
                "description": format!("{} Defaults to \"{defaults}\".", e.about),
                "anyOf": [
                    { "type": "string" },
                    { "type": "array", "items": { "type": "string" } },
                ],
            });
            (e.name.to_owned(), value)
        })
        .collect();
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": FILE_NAME,
        "description": "Keys for the aqx TUI. Each command takes a key, like \"o\", \"O\", \"ctrl+p\" or \"pagedown\", or a list of keys. An empty list unbinds it.",
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    });
    serde_json::to_string_pretty(&schema).expect("a schema serializes")
}

pub fn keymap_path() -> Option<PathBuf> {
    crate::config::default_home().map(|home| home.join(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(keymap: &Keymap, code: KeyCode, modifiers: KeyModifiers) -> Option<Command> {
        keymap.command(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn a_key_matches_however_the_terminal_reports_shift() {
        let keymap =
            Keymap::parse("stop = \"shift+s\"\nfocus = \"shift+tab\"\nzoom = \"ctrl+z\"").unwrap();
        assert_eq!(
            press(&keymap, KeyCode::Char('S'), KeyModifiers::SHIFT),
            Some(Command::Stop)
        );
        assert_eq!(
            press(&keymap, KeyCode::Char('S'), KeyModifiers::NONE),
            Some(Command::Stop)
        );
        assert_eq!(press(&keymap, KeyCode::Char('s'), KeyModifiers::NONE), None);
        assert_eq!(
            press(&keymap, KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(Command::Focus)
        );
        assert_eq!(
            press(&keymap, KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(Command::Zoom)
        );
        assert_eq!(press(&keymap, KeyCode::Char('z'), KeyModifiers::NONE), None);
        assert_eq!(keymap.key(Command::Stop), "S");
        assert_eq!(keymap.key(Command::Zoom), "ctrl+z");
    }

    #[test]
    fn a_keys_file_with_a_mistake_says_what_is_wrong() {
        for (text, error) in [
            ("jump = \"j\"", "\"jump\" isn't a command"),
            ("stop = 3", "stop takes a key or a list of keys"),
            (
                "stop = \"hyper+s\"",
                "stop: \"hyper+s\" has an unknown modifier \"hyper\"",
            ),
            ("stop = \"pgup\"", "stop: \"pgup\" isn't a key aqx knows"),
            (
                "stop = \"shift+1\"",
                "stop: \"shift+1\" needs the character shift types, like \"!\" for shift+1",
            ),
            ("stop = \"r\"", "r is bound to both stop and restart"),
            (
                "detach = \"ctrl+c\"",
                "detach: ctrl+c always asks to end the session",
            ),
        ] {
            assert_eq!(Keymap::parse(text).unwrap_err(), error, "{text}");
        }
        assert!(
            Keymap::parse("stop = \"r\"\nrestart = \"R\"").is_ok(),
            "a swap is fine"
        );
    }

    #[test]
    fn a_pressed_key_saves_as_text_that_reads_back_as_the_same_key() {
        for text in [
            "s",
            "S",
            "+",
            "ctrl++",
            "ctrl+s",
            "ctrl+S",
            "alt+x",
            "space",
            "shift+tab",
            "ctrl+shift+tab",
            "shift+up",
            "pagedown",
            "f5",
            "?",
        ] {
            let key = Key::parse(text).unwrap();
            let saved = key.name().unwrap();
            assert_eq!(Key::parse(&saved).unwrap(), key, "{text} saved as {saved}");
        }
        let pressed = Key::of(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
        assert_eq!(pressed.name().as_deref(), Some("S"));
    }

    #[test]
    fn rebinding_saves_the_file_and_keeps_what_else_it_says() {
        let dir = std::env::temp_dir().join(format!("aqx-rebind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FILE_NAME);
        let mut keymap = Keymap {
            path: Some(path.clone()),
            ..Keymap::default()
        };
        let ctrl_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        keymap = keymap.rebind(Command::Stop, Some(ctrl_s)).unwrap();
        assert_eq!(keymap.key(Command::Stop), "ctrl+s");
        assert!(!keymap.is_default(Command::Stop) && keymap.is_default(Command::Restart));

        std::fs::write(
            &path,
            "# my keys\nstop = \"ctrl+s\" # easier\nzoom = \"Z\"\n",
        )
        .unwrap();
        let r = KeyEvent::from(KeyCode::Char('r'));
        assert_eq!(
            keymap.rebind(Command::LogIn, Some(r)).unwrap_err(),
            "r is bound to both log_in and restart"
        );
        keymap = keymap.rebind(Command::Zoom, None).unwrap();
        assert_eq!(keymap.key(Command::Zoom), "z");
        let k = KeyEvent::from(KeyCode::Char('k'));
        keymap = keymap.rebind(Command::Stop, Some(k)).unwrap();
        assert_eq!(keymap.key(Command::Stop), "k");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# my keys\nstop = \"k\" # easier\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_schema_names_every_command() {
        let schema: serde_json::Value = serde_json::from_str(&schema()).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(properties.len(), COMMANDS.len());
        assert_eq!(
            properties["type_script"]["description"],
            "Type a script path and args to run. Defaults to \"O\"."
        );
        assert_eq!(schema["additionalProperties"], false);
    }
}
