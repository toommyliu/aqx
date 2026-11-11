# Run accounts from the browser

`aqx web` serves a web UI for the accounts in `aqx.toml` at `http://127.0.0.1:4970`. It logs accounts in and out, picks and runs scripts, asks for their args, switches servers, accepts drops, registers drops and quests for the running script, edits Skua options, and shows each account's log and lists. It also edits `aqx.toml` and lets you change its keys.

## Build it

Building aqx builds the UI and embeds it in the binary. It needs Node 24 or newer.

```sh
cargo install --path crates/aqx
```

An aqx built without Node still runs, but `aqx web` answers every page with a note to build the UI first.

## Start it

```sh
aqx web [-c PATH] [--port N] [--up] [--tailscale]
```

`-c` picks the config, the same way it does for `aqx up`. `aqx web` attaches to that config's session when one is running. When none is, the page lists the accounts as offline, and the first account you log in or run a script on starts the session. `--up` starts it right away instead, and logs in the accounts with `auto_start` on, as `aqx up -d` does. With a session already running, `--up` changes nothing. Stopping `aqx web` leaves the session running either way. The session still ends on its own once every account is offline, and `aqx web` keeps running so you can start it again. Each account's log keeps its last lines from the session that ended, and the overview shows each account's last line, until the next session starts. The Session menu restarts the session, which applies changes to `aqx.toml`, and logs everyone out.

`aqx web` listens on `127.0.0.1` only and has no login. It refuses requests whose `Host` isn't `localhost`, `127.0.0.1`, `[::1]` or a `*.ts.net` name.

To record an account's packets once it's online, press the record button in the top right corner of the account, or pick Record packets from the menu you get by right-clicking the account in the list or clicking its `⋯`. While it records, the button reads *rec*, a red dot shows beside the account in the list, and the account's log says which file aqx writes to. The account also gets a Packets tab that lists its last 1000 sent and 1000 received packets, each marked sent or received. Each packet takes one line, cut short when it's long, and the copy button beside a packet copies all of it. Press the button again to stop. `aqx ctl ACCOUNT record` does the same from a terminal; see [Run sessions](config.md#run-sessions).

To queue a script, press the chevron next to Run and pick Add to queue, then pick a script the same way. It also works on ticked accounts, and it never asks first, since nothing stops. Once an account has queued scripts, an Up next list shows below its actions. `↑` and `↓` reorder it, `×` removes a script, Start runs the first one on an idle account, Skip to next stops the running script and runs the first one, and Clear empties the list. [Queue scripts](config.md#queue-scripts) says when the queue moves on.

While a script runs, a switch at the left of the Drops tab's toolbar flips between pending drops and the drops the script registered, and the Quests tab's switch flips between accepted and registered quests. Registered means `drops.register` and `quests.register` in a TypeScript script, `Bot.Drops.Add` and `Bot.Quests.RegisterQuests` in a Skua script. On the registered list, type a drop's name or item ID and press Register to add one. For a quest, give its ID and, if you want a particular reward, the reward's item ID. `×` unregisters an entry. An entry registered by ID shows *Unknown name* until the account has that item or has the quest accepted. Your changes go to the running script, as if it had made the call itself, so they last until the script stops or restarts. A Skua script keeps its registered drops when aqx restarts it after a reconnect, as it does under Skua. A Skua script only picks up registered drops once its drop grabber is running, as in Skua.

The eye button in the header shows your accounts as `account 1`, `account 2` and so on, your home folder as `~`, and room numbers as `•••••` (so `battleon-12345` reads `battleon-•••••`), so you can share a screenshot. The config editor still shows the real names, since you edit them there.

## Reach it from another device

`--tailscale` puts it on your tailnet with [Tailscale Serve](https://tailscale.com/kb/1312/serve), over HTTPS on the same port:

```sh
aqx web --tailscale
```

It prints `https://<machine>.<tailnet>.ts.net:4970`, which opens on any device in your tailnet. If a route to aqx web already exists, such as one on port 443, it prints that link instead and adds nothing. It needs the `tailscale` command on your `PATH`. The route stays after `aqx web` stops, and every `aqx web` start prints each Tailscale Serve route to its port while tailscale is connected, flag or not, so you only pass `--tailscale` once. If that port on your tailnet already serves something else, `--tailscale` leaves it alone and asks for another `--port`. `tailscale serve --https=4970 --set-path=/ off` takes the route down. Anyone who can reach that address can drive your accounts, so keep it off [Funnel](https://tailscale.com/kb/1223/funnel).

## Edit aqx.toml

Config in the header opens the config editor, and Add account under the account list opens it on a new account. To open it on one account, right-click that account in the list, or click the `⋯` that shows when you point at it, and pick Edit config. The config editor has three tabs. Accounts shows each account's login, script and script args, then only the settings that account sets itself. For a Skua script, script args become script options: one row for each of the script's options, starting on Shared, the value every account that runs the script uses. A value you pick there applies to that account only. Override a setting adds one, and its `×` hands the setting back to Defaults. Defaults lists every setting, and a setting left unset uses aqx's default. Server picks from the live servers, and Choose picks an account's script the same way Run does. Raw edits the file as text, beside a list of every key `aqx.toml` takes. Saving from any tab keeps your comments and only rewrites the lines you changed. The editor never shows a saved password; leave the field empty to keep it.

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

`mod` is `⌘` on a Mac and `ctrl` elsewhere. The account numbers can't be changed. Restart, switch server, log out and log in without the script have no key until you give them one. To log in without the script from an account's page, use the chevron next to Log in. The keys and the buttons on an account's page act on that account only. To act on several accounts, pick Select in the account list and tick them, or drag across them with the mouse, then use the bar at the bottom of the page. A drag that starts on a ticked account unticks the ones it crosses. It has every action an account's page has. Each one acts on the ticked accounts it applies to: Log in on the offline ones, Stop on the ones running a script, and so on. Run, Stop and Restart ask before they act on more than one account. Log out always asks.

Everything also works with the keyboard alone. `tab` moves between areas, `↑` `↓` move through lists, `←` `→` move between tabs, `enter` opens, and `esc` closes a dialog.

## Work on the UI

`npm --prefix web run dev` serves the UI from source at `http://127.0.0.1:5173` and sends API calls to `aqx web` on port 4970. Set `AQX_PORT` to use another port.
