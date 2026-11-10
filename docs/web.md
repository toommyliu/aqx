# Run accounts from the browser

`aqx web` serves a web UI for the accounts in `aqx.toml` at `http://127.0.0.1:4970`. It logs accounts in and out, picks and runs scripts, asks for their args, switches servers, accepts drops, edits Skua options, and shows each account's log and lists. It also edits `aqx.toml` and lets you change its keys.

## Build it

The UI is built into the `aqx` binary, so build it before you build aqx. It needs Node 24 or newer.

```sh
npm --prefix web ci
npm --prefix web run build
cargo install --path crates/aqx
```

An aqx built without these steps still runs, but `aqx web` answers every page with a note to build the UI first.

## Start it

```sh
aqx web [-c PATH] [--port N] [--up]
```

`-c` picks the config, the same way it does for `aqx up`. `aqx web` attaches to that config's session when one is running. When none is, the page lists the accounts as offline, and the first account you log in or run a script on starts the session. `--up` starts it right away instead, and logs in the accounts with `auto_start` on, as `aqx up -d` does. With a session already running, `--up` changes nothing. Stopping `aqx web` leaves the session running either way. The session still ends on its own once every account is offline, and `aqx web` keeps running so you can start it again. Each account's log keeps its last lines from the session that ended, and the overview shows each account's last line, until the next session starts. The Session menu restarts the session, which applies changes to `aqx.toml`, and logs everyone out.

`aqx web` listens on `127.0.0.1` only and has no login. It refuses requests whose `Host` isn't `localhost`, `127.0.0.1`, `[::1]` or a `*.ts.net` name.

The eye button in the header shows your accounts as `account 1`, `account 2` and so on, your home folder as `~`, and room numbers as `•••••` (so `battleon-12345` reads `battleon-•••••`), so you can share a screenshot. The config editor still shows the real names, since you edit them there.

## Reach it from another device

Use [Tailscale Serve](https://tailscale.com/kb/1312/serve) to put it on your tailnet:

```sh
tailscale serve --bg 4970
```

Then open `https://<machine>.<tailnet>.ts.net` on any device in your tailnet. `tailscale serve reset` takes it down again. Anyone who can reach that address can drive your accounts, so keep it off [Funnel](https://tailscale.com/kb/1223/funnel).

## Edit aqx.toml

Config in the header opens the config editor, and Add account under the account list opens it on a new account. To open it on one account, right-click that account in the list, or click the `⋯` that shows when you point at it, and pick Edit config. The config editor has three tabs. Accounts shows each account's login, script and script args, then only the settings that account sets itself. Override a setting adds one, and its `×` hands the setting back to Defaults. Defaults lists every setting, and a setting left unset uses aqx's default. Server picks from the live servers, and Choose picks an account's script the same way Run does. Raw edits the file as text, beside a list of every key `aqx.toml` takes. Saving from any tab keeps your comments and only rewrites the lines you changed. The editor never shows a saved password; leave the field empty to keep it.

aqx checks the file before saving it and refuses a change that wouldn't load. It also refuses to save when the file changed on disk after you opened it. New accounts join the running session within 2 seconds. Changes to accounts already in the session apply after it restarts. Until then, the header shows *config changed*, whether you edited the file here or somewhere else.

## Keys

Press `?` to list the keys on a computer. Pick Change on a row, then press the new key. Keys you change show in blue, and aqx saves them in its data folder (`~/Library/Application Support/aqx/web-keys.json` on macOS), so every browser gets them.

| Key | Does |
| --- | --- |
| `i` | Log in, then run the account's script |
| `o` | Pick a script and run it |
| `s` | Stop the script |
| `p` | Pause or resume the script |
| `j`, `k` | Next or previous account |
| `x` | Tick or untick the account under the cursor, or the open one |
| `shift`+`↑` `↓` | Tick a run of accounts in the list |
| `mod`+`a` | Select every account, or none when they're all selected. In the log, the lists and text fields it selects text instead |
| `mod`+`1`–`9` | Open accounts 1 to 9, numbered in `aqx.toml` order. Hold `mod` to see each account's number. Only in the installed app; a browser tab keeps these keys for switching tabs |
| `/` | Search the log or list |
| `esc` | Clear the search, then leave select mode |
| `?` | List and change the keys |

`mod` is `⌘` on a Mac and `ctrl` elsewhere. The account numbers can't be changed. Restart, switch server and log out have no key until you give them one. The keys and the buttons on an account's page act on that account only. To act on several accounts, pick Select in the account list and tick them, or drag across them with the mouse, then use the bar at the bottom of the page. A drag that starts on a ticked account unticks the ones it crosses. It has every action an account's page has. Each one acts on the ticked accounts it applies to: Log in on the offline ones, Stop on the ones running a script, and so on. Run, Stop and Restart ask before they act on more than one account. Log out always asks.

Everything also works with the keyboard alone. `tab` moves between areas, `↑` `↓` move through lists, `←` `→` move between tabs, `enter` opens, and `esc` closes a dialog.

## Work on the UI

`npm --prefix web run dev` serves the UI from source at `http://127.0.0.1:5173` and sends API calls to `aqx web` on port 4970. Set `AQX_PORT` to use another port.
