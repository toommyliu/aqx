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
