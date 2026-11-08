# mira

mira is a headless AQW client written in Rust. You run and script your accounts from a CLI, a TUI, or a macOS desktop app. Scripts are TypeScript files, and a compatibility layer runs most Skua scripts unchanged. Inspired by [aqw-python](https://github.com/Froztt13/aqw-python).
## Install

```sh
git clone https://github.com/toommyliu/mira.git
cd mira
cargo install --path crates/mira
mira init
```

- [Run accounts from the TUI](docs/tui.md)
- [Write a mira script](docs/writing-scripts.md)
- [Run Skua scripts](docs/skua.md). Skua support also needs the .NET SDK and the Skua submodule.

## How it works

mira doesn't open a game window or load Flash. It talks to the AQW server directly, the same way the game client does.

Skua scripts normally need Skua, which runs only on Windows. mira runs them in a small .NET helper instead and performs actions in the game on the helper's behalf.
