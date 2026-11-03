# aqx

A headless AQW client written in Rust that runs bot scripts written in TypeScript. Inspired by [aqw-python](https://github.com/Froztt13/aqw-python).

Skua scripts (`.cs`) run through the shim in `skua/` on the `skua/host` branch, which compiles them against Skua's interfaces and forwards each game call to aqx. The shim is experimental.

## CLI

```
aqx [ACCOUNT...] [-c FILE] [--script FILE [--arg KEY=VALUE]...] [--redact]
    Open the TUI. Without accounts, a new session logs in every account.
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
