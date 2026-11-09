# mira

mira is a headless AQW client written in Rust. You run and script your accounts from a CLI, a TUI, or a web UI. Scripts are TypeScript files, and a compatibility layer runs most Skua scripts unchanged. Inspired by [aqw-python](https://github.com/Froztt13/aqw-python).

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) 1.85 or newer
- [Node](https://nodejs.org/en/download) 24 or newer, to build the web UI
- The [.NET 10 SDK](https://dotnet.microsoft.com/download/dotnet/10.0), only to run Skua scripts

## Install

```sh
git clone https://github.com/toommyliu/mira.git
cd mira

# Build the web UI. cargo install embeds it in the mira binary.
npm --prefix web ci
npm --prefix web run build

# Optional: fetch Skua to run Skua scripts.
git submodule update --init --depth 1 skua/vendor/Skua

cargo install --path crates/mira

# Set up a scripts folder with mira.toml, in ~/mira by default.
mira init
```

## Docs

- [Run accounts from the TUI](docs/tui.md)
- [Run accounts from the browser](docs/web.md)
- [Write a mira script](docs/writing-scripts.md)
- [Run Skua scripts](docs/skua.md)

## How it works

mira doesn't open a game window or load Flash. It talks to the AQW server directly, the same way the game client does.

Skua scripts normally need Skua, which runs only on Windows. mira runs them in a small .NET helper instead and performs actions in the game on the helper's behalf.
