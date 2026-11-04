# aqx

A headless AQW client written in Rust that runs bot scripts written in TypeScript. Inspired by [aqw-python](https://github.com/Froztt13/aqw-python).

Skua scripts (`.cs`) run through the shim in `skua/` on the `skua/host` branch, which compiles them against Skua's interfaces and forwards each game call to aqx. The shim is experimental. To set it up, see [Run Skua scripts](docs/skua.md).

## Install

aqx runs on macOS and Linux. To build it, you need Rust 1.89 or later and a C compiler.

```
cargo install --locked --git https://github.com/toommyliu/aqx aqx
```

[Get started](docs/getting-started.md) shows how to install those tools, add an account, and run a first script.

The terminal UI needs a terminal that shows UTF-8 and the 16 standard colors, with a font that has box-drawing and braille characters. The mouse is optional: the UI uses it for clicks, scrolling, and resizing the account list. `aqx run`, `aqx serve`, and `aqx logs` print plain lines, so they work over SSH and when piped to a file.

## CLI

```
aqx [ACCOUNT...] [-c FILE] [--script FILE [--arg KEY=VALUE]...] [--redact]
    Open the TUI. Without accounts, a new session logs in every account with autologin on.
    --script runs that script instead of each account's own; --arg feeds script.args().
    --redact shows accounts as account 1, account 2, ... and the home directory as ~.

aqx run [SCRIPT] [--arg KEY=VALUE]... [-a NAME] [-c FILE] [--server NAME]
        [--idle] [--record FILE] [--client-version VERSION] [--json]
    Log one account in and print what it does until its script ends,
    or until Ctrl-C without one. --idle logs in without running a script.
    --record writes every packet to FILE as JSON lines.

aqx serve [ACCOUNT...] [-c FILE] [--script FILE [--arg KEY=VALUE]...] [--json | --quiet]
    Run the session in the foreground, for systemd.

aqx down [-c FILE]            Log out every account and end the session.
aqx ps                        List running aqx processes and their accounts.
aqx logs [ACCOUNT] [-f] [--all]
    Print an account's recent log. -f follows it; --all adds moves, kills,
    drops, items and quests.

aqx ctl ACCOUNT ACTION        Control an account in a running aqx process.
    login                     Log in and run the account's own script.
    run SCRIPT [--arg K=V]... Run a script, stopping the current one. Logs in if offline.
    stop | restart            Stop the script, or run the last one again.
    pause | resume            Pause or resume the script.
    relogin | logout          Log out and back in, or log out.

aqx servers [-c FILE]         List the game servers.
aqx view FILE [--cmd NAME]... [--full]
    Print a --record file one packet per line. --cmd keeps only those
    commands; --full stops cutting lines at 160 characters.

aqx schema                    Print the JSON Schema of aqx.toml.
aqx init [DIR]                Write aqx.d.ts, tsconfig.json, a sample script and an
                              aqx.toml. Rerun after upgrading to update aqx.d.ts.
aqx check SCRIPT...           Compile scripts and their imports without logging in.
```

## Disclaimer

aqx is not affiliated with or endorsed by Artix Entertainment. Botting can break AQW's terms of service, use at your own risk.
