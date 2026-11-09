# Run accounts from the TUI

`aqx` with no command opens the TUI for the accounts in `aqx.toml`. You log accounts in, run scripts on them, and watch their logs from one terminal.

The accounts live in a session that runs in the background. The first `aqx` starts it, and later ones attach to it. `q` detaches and leaves everything running. `Q` logs every account out and ends the session. The session also ends on its own once every account is offline after something logged in, and the TUI then prints the last few lines of each account's log.

`aqx ACCOUNT...` logs in only the accounts you name. Without names, a new session logs in every account with `auto_start` on. `aqx ACCOUNT... --script PATH [--arg KEY=VALUE]...` runs that script on the named accounts instead of their own. `aqx tui` does the same as `aqx`. `aqx --redact` shows your accounts as `account 1`, `account 2` and so on, and your home folder as `~`, so you can share a screenshot.

## Write aqx.toml

`aqx init` sets up `~/aqx`, or the folder you name. It writes `aqx.d.ts`, a `tsconfig.json` that points at it, a sample script, `aqx.schema.json` and `keys.schema.json` for editor completion, and a `.gitignore` that keeps `aqx.toml` out of git. In a terminal it also asks for one account and writes an `aqx.toml` that runs the sample. Running it again updates `aqx.d.ts` and the schemas and leaves your other files alone. `aqx schema` prints the schema.

```toml
[defaults]
server = "Twilly"

[[account]]
user = "player1"
password = "change-me"
script = "farm/void-aura.ts"
args = { target = 7500, fast = true }

[[account]]
name = "healer"
user = "player2"
password = "change-me"
server = "Yorumi"
auto_start = false
```

aqx looks for `./aqx.toml`, then `~/aqx/aqx.toml`. Set `AQX_HOME` to use another folder in place of `~/aqx`, or pass `-c PATH` to read a specific file. The file holds passwords, so run `chmod 600` on it. aqx warns when other users can read it.

`[defaults]` takes every key below except `user`, `password`, `name`, `script` and `args`, and applies it to each account. An `[[account]]` can override any of them except `client_version`, which only `[defaults]` sets.

| Key | Meaning |
| --- | --- |
| `user`, `password` | The login. Required. |
| `name` | What aqx calls the account. Defaults to `user`. |
| `server` | Server to join, as `aqx servers` lists it. Required here or in `[defaults]`. |
| `client_version` | Log in with exactly this client version instead of the server's minimum. |
| `script` | Script to run each time the account logs in, relative to `aqx.toml`. |
| `args` | Arguments for that script. Values are strings, numbers or booleans. |
| `auto_start` | Log the account in when a session starts without naming accounts. Defaults to `true`. |
| `room` | Room number to join. Scripts read it as `settings.room`. |
| `auto_relogin` | Log back in to `server` after a failed login, a lost connection or a kick, then start the script again. A wrong password isn't retried. Defaults to `true`. |
| `relogin_delay` | The wait before the first try, like `"5s"`. It doubles after each failed try, up to 5 minutes, and is never shorter than a kick's lockout. A connection that lasts 5 minutes starts the wait over. At least `"4s"`. Defaults to `"5s"`. |
| `relogin_attempts` | How many failed tries in a row before the account goes offline. No limit unless set. |
| `restart_on_error` | Run the script again when it throws, after 10 seconds, doubling up to 5 minutes. A run that lasts 5 minutes starts the wait over. Defaults to `false`. |
| `stall_after` | How long a running script can go without progress before the account counts as stalled, like `"90s"`, `"5m"` or `"1h"`. Progress is a kill, an item change, a quest turn-in or a map join. Defaults to `"5m"`. |
| `on_stall` | What a stall does: `warn` logs a warning, `restart` restarts the script, and `relogin` logs out, back in, and starts the script again. Defaults to `warn`. |

Script paths you type in the TUI also resolve relative to `aqx.toml`. The session checks the file every 2 seconds and adds any `[[account]]` with a new name, offline. Edits to accounts that are already in the session, `auto_start` included, take effect when it restarts, and each changed account's log says so. Run `aqx down`, then `aqx`, to apply them.

## Keys

The screen has two boxes: the account list on the left, and the selected account on the right, with its details above its log or one of its lists. `tab` moves focus between them, and the focused box has a blue border. `↑` `↓`, `space`, `a`, `enter` and `/` act on the focused pane. With one account there's no account list, so they act on the main pane.

These keys act on the marked accounts, or on the selected one when nothing is marked:

| Key | Does |
| --- | --- |
| `i` | Log in, then run the account's `script` |
| `o` | Pick a script and run it. The list opens on the selected account's script. The files tab lists the folders and scripts in one folder, starting at the `scripts` folder next to `aqx.toml`. `←` goes up a folder, and `→` or `enter` opens the selected one. To jump somewhere else, press `/` and type a path that starts with `/`, `~/`, `./` or `../`, or paste one. The list shows that folder, narrowed to names that start with whatever follows the last `/`. The Skua tab lists the Skua library. `tab` switches between them. `→` on a Skua script edits its options, and `u` on the Skua tab downloads or updates the library. See [Run Skua scripts](skua.md). |
| `O` | Type a script path and `key=value` args. `tab` completes, and `↑` recalls an earlier run. |
| `s`, `r` | Stop or restart the script. Stopping also leaves combat. |
| `p` | Pause the script, or resume it. A pause lets the current step finish, such as a kill in progress, then holds the script. Skua scripts can't pause. |
| `c` | Log in to another server, then run the script that was running. The list marks the servers that are offline or full, and members-only servers when the account is online and free, and won't switch to them. The account stays on the new server until it logs out, auto relogin included. Logging out, or a script's `auth.logout()`, sends the next login back to the `server` in `aqx.toml`. |
| `x` | Log out |

These act on the focused pane:

| Key | Does |
| --- | --- |
| `tab` | Move focus between the account list and the main pane |
| `↑` `↓` | Pick an account, or scroll the main pane. On the dashboard, `←` `→` move between cards too. |
| `space`, `a` | Mark one account or drop, or all of them. `a` unmarks them when they're all marked. |
| `enter` | Focus the main pane, or accept drops. On the dashboard, open the account. |
| `/` | Search the focused pane: accounts by name, lines in the log, or rows in a list. `enter` keeps the search, and `esc` clears it. |
| `esc` | Clear the marked drops, then the search, then go back to the log and the account list, then unmark the accounts |

These work anywhere:

| Key | Does |
| --- | --- |
| `g` | Show every account as a card |
| `d` | Show more of the selected account |
| `z` | Hide the account list and the side panel, or show them again |
| `<` `>` | Narrow or widen the account list. Dragging its edge works too. |
| `PgUp` `PgDn` | Scroll the main pane a page |
| `Home` `End` | Scroll the main pane to the top, or to the newest lines |
| `q` | Detach, leaving the accounts running |
| `Q`, `ctrl+c` | Ask to end the session. In that dialog `enter` logs everyone out, `q` or a second `ctrl+c` detaches, and `esc` stays. The TUI stays open while the accounts log out, then closes as the session ends. |
| `?` | List every key |

A stalled account shows `!` in the sidebar, and a paused one shows `‖`.

### Switch views

The area under the account shows its log or one of its lists. Each view has a number, and the number opens it directly:

| Key | View |
| --- | --- |
| `1` | Log |
| `2` | Players in the cell |
| `3` | Drops waiting to be accepted |
| `4` | Accepted quests and turn-in progress |
| `5` | Temporary items |
| `6` | Inventory |
| `7` | Bank, once a script has opened it |
| `8` | Faction ranks |

`esc` goes back to the log. A search stays on when you pick another account or move focus, and opening another view clears it. The side panel labels each list with its number, and clicking a list there opens it. With the side panel hidden (`z`), the divider shows every view as a tab instead.

The log sorts lines into channels: `aqx`, `script`, `server`, `chat` and `game`. The divider lists them, with the one shown highlighted. `[` and `]` move one channel left or right, and stop at the ends. `main`, the default, shows everything except chat and game events. `all` shows every channel. Clicking a channel shows it.

Opening the drops list focuses the main pane. There `↑` `↓` move, `space` marks a drop, `a` marks them all, and `enter` accepts the marked drops, or the one under the cursor.

### Change the keys

Press `?`, then `e`, to change keys in the TUI. Pick a command and press `enter`, then the new key. `backspace` puts the default back, and keys that differ from the default show in blue. A key that's already taken shows an error and changes nothing, so to swap two keys, move one to a spare key first. `esc` cancels a change, so it can't be picked this way. `esc` in the list goes back to `?`. aqx saves each change to `keys.toml` right away and leaves the rest of the file, comments included, as it was. A key you pick there replaces every key that command had.

You can also write `keys.toml` yourself. It lives in `~/aqx`, or in `AQX_HOME` when it's set. Each line gives a command a key, or a list of keys. A command you leave out keeps its default, and an empty list unbinds it:

```toml
#:schema ./keys.schema.json
up = ["k", "up"]
down = ["j", "down"]
pick_script = "ctrl+o"
details = []
```

A key is a character such as `o`, `O` or `?`, or one of `up`, `down`, `left`, `right`, `enter`, `esc`, `tab`, `space`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown` and `f1` to `f12`. Put `ctrl+`, `alt+` or `shift+` in front for a combination. `shift+` works with letters and `tab`; for other shifted characters, write the character itself, like `!`. `aqx` refuses to open the TUI when the file names a command it doesn't know, a key it can't read, or one key for two commands, and says which. `ctrl+c` always asks to end the session, and keys inside the script picker, the run prompt and other dialogs stay the same. `?`, the footer and the side panel show the keys you set. aqx reads the file when the TUI opens, so edits you make by hand show up the next time you open it.

`aqx init` writes `keys.schema.json`, so an editor can complete the command names:

| Command | Default | Command | Default |
| --- | --- | --- | --- |
| `focus` | `tab` | `view_log` | `1` |
| `up` | `up` | `view_players` | `2` |
| `down` | `down` | `view_drops` | `3` |
| `left` | `left` | `view_quests` | `4` |
| `right` | `right` | `view_temp` | `5` |
| `mark` | `space` | `view_inventory` | `6` |
| `mark_all` | `a` | `view_bank` | `7` |
| `open` | `enter` | `view_factions` | `8` |
| `search` | `/` | `previous_channel` | `[` |
| `back` | `esc` | `next_channel` | `]` |
| `log_in` | `i` | `dashboard` | `g` |
| `log_out` | `x` | `details` | `d` |
| `pick_script` | `o` | `zoom` | `z` |
| `type_script` | `O` | `narrower_sidebar` | `<` |
| `stop` | `s` | `wider_sidebar` | `>` |
| `restart` | `r` | `page_up` | `pageup` |
| `pause` | `p` | `page_down` | `pagedown` |
| `switch_server` | `c` | `oldest` | `home` |
| `help` | `?` | `newest` | `end` |
| `detach` | `q` | `end_session` | `Q` |

### Script args

Before a run starts, aqx asks in the footer for each arg the script declares that you didn't type. It shows the type it expects and rejects an answer that doesn't fit. An optional arg says so, and shows its default when the script writes it as a plain value. `enter` on an empty answer skips it, so the script uses its default. With several accounts marked, it asks each account in turn, so each can get its own answer. Press `tab` before the first answer to give one answer for all of them; aqx keeps that choice for the next run. aqx remembers your answers until you quit and offers them the next time. Typing replaces an offered answer, and `backspace` edits it.

aqx keeps the scripts you run in its data folder (`~/Library/Application Support/aqx/history` on macOS), one entry per script with the args you last typed for it, and `↑` walks back through them, newest first, in later sessions. After them come the scripts your accounts are running, even ones started with `aqx ctl` or from another terminal. Answers to asked args stay out of that file.

## Sessions from the shell

Each config gets its own session, so accounts from different `aqx.toml` files can run at the same time.

| Command | Does |
| --- | --- |
| `aqx up [ACCOUNT...] [-c PATH]` | Start the session in the background without opening the TUI |
| `aqx serve [ACCOUNT...] [-c PATH] [--json \| --quiet]` | Run the session in the foreground and print its log, for systemd. `ctrl+c` logs everyone out; a second one exits at once. |
| `aqx ps [--json]` | List every running session's accounts: state, script, where, uptime and status |
| `aqx logs [ACCOUNT] [-f] [--all]` | Print an account's recent log, or every account's when only one session runs. `-f` keeps printing, and `--all` adds game lines. |
| `aqx ctl ACCOUNT login\|logout\|stop\|restart\|pause\|resume\|relogin` | Drive an account in whichever session has it |
| `aqx ctl ACCOUNT run SCRIPT [--arg KEY=VALUE]...` | Run a script, stopping the one running. Logs the account in if it's offline. |
| `aqx down [-c PATH]` | Log out every account and end the session |

`ACCOUNT` is a name or a username. Sessions keep their sockets, locks and logs in aqx's data folder (`~/Library/Application Support/aqx/sessions` on macOS), which only you can open. Set `AQX_SESSIONS` to use another folder. When `aqx` attaches to a session that was already running, the TUI says when that session started, and warns if `aqx.toml` changed after it. When the running session is from another build of aqx, `aqx` asks whether to restart it with this one, and restarts it without asking when the two builds can't talk to each other. A restart logs the online accounts out and back in, and their scripts start again.

## Other tools

| Command | Does |
| --- | --- |
| `aqx check SCRIPT...` | Compile scripts and the files they import, without logging in or running them |
| `aqx skua update\|list [QUERY]\|path` | Download or update the Skua script library, search it, or print its folder. See [Run Skua scripts](skua.md). |
| `aqx run -a ACCOUNT [SCRIPT] [--arg KEY=VALUE]...` | Log one account from `aqx.toml` in, run its script or the one named, and log out when it ends |
| `aqx run ... --record FILE` | Also write every packet to `FILE` as JSON lines, with the token and personal fields redacted |
| `aqx view FILE [--cmd NAME]... [--full]` | Print a recording one packet per line, only the named commands if any, cut at 160 characters unless `--full` |
