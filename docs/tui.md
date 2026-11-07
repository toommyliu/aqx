# Run accounts from the TUI

`mira` with no command opens the TUI for the accounts in `mira.toml`. You log accounts in, run scripts on them, and watch their logs from one terminal.

The accounts live in a session that runs in the background. The first `mira` starts it, and later ones attach to it. `q` detaches and leaves everything running. `Q` logs every account out and ends the session. The session also ends on its own once every account is offline after something logged in, and the TUI then prints the last few lines of each account's log.

`mira ACCOUNT...` logs in only the accounts you name. Without names, a new session logs in every account with `autologin` on. `mira ACCOUNT... --script PATH [--arg KEY=VALUE]...` runs that script on the named accounts instead of their own. `mira tui` does the same as `mira`. `mira --redact` shows your accounts as `account 1`, `account 2` and so on, and your home folder as `~`, so you can share a screenshot.

## Write mira.toml

`mira init` sets up `~/mira`, or the folder you name. It writes `mira.d.ts`, a `tsconfig.json` that points at it, a sample script, `mira.schema.json` for editor completion, and a `.gitignore` that keeps `mira.toml` out of git. In a terminal it also asks for one account and writes a `mira.toml` that runs the sample. Running it again updates `mira.d.ts` and the schema and leaves your other files alone. `mira schema` prints the schema.

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
autologin = false
```

mira looks for `./mira.toml`, then `~/mira/mira.toml`. Set `MIRA_HOME` to use another folder in place of `~/mira`, or pass `-c PATH` to read a specific file. The file holds passwords, so run `chmod 600` on it. mira warns when other users can read it.

`[defaults]` takes every key below except `user`, `password`, `name`, `script` and `args`, and applies it to each account. An `[[account]]` can override any of them.

| Key | Meaning |
| --- | --- |
| `user`, `password` | The login. Required. |
| `name` | What mira calls the account. Defaults to `user`. |
| `server` | Server to join, as `mira servers` lists it. Required here or in `[defaults]`. |
| `client_version` | Log in with exactly this client version instead of the server's minimum. |
| `script` | Script to run each time the account logs in, relative to `mira.toml`. |
| `args` | Arguments for that script. Values are strings, numbers or booleans. |
| `autologin` | Log the account in when a session starts without naming accounts. Defaults to `true`. |
| `room` | Room number to join. Scripts read it as `settings.room`. |
| `api` | Host that takes the login. Defaults to `https://game.aq.com`. |
| `reconnect` | Seconds to wait before logging back in after a failed login, a lost connection or a kick, doubling up to 5 minutes. A wrong password isn't retried. Off unless set. |
| `reconnect_attempts` | How many times in a row to try before the account goes offline. Defaults to 5. Needs `reconnect`. |
| `restart_on_error` | Run the script again when it throws, after 10 seconds, doubling up to 5 minutes. A run that lasts 5 minutes starts the wait over. Defaults to `false`. |
| `stall_after` | How long a running script can go without progress before the account counts as stalled, like `"90s"`, `"5m"` or `"1h"`. Progress is a kill, an item change, a quest turn-in or a map join. Defaults to `"5m"`. |
| `on_stall` | What a stall does: `warn` logs a warning, `restart` restarts the script, and `relogin` logs out, back in, and starts the script again. Defaults to `warn`. |

Script paths you type in the TUI also resolve relative to `mira.toml`. The session checks the file every 2 seconds and adds any `[[account]]` with a new name, offline. Edits to accounts that are already in the session, `autologin` included, take effect when it restarts, and each changed account's log says so. Run `mira down`, then `mira`, to apply them.

## Keys

Keys act on the marked accounts, or on the selected one when nothing is marked.

| Key | Does |
| --- | --- |
| `i` | Log in, then run the account's `script` |
| `o` | Run a script: type a path and `key=value` args, `tab` completes, `↑` recalls an earlier run |
| `s`, `r` | Stop or restart the script. Stopping also leaves combat. |
| `p` | Pause the script, or resume it. A pause lets the current step finish, such as a kill in progress, then holds the script. Skua scripts can't pause. |
| `x` | Log out |
| `↑` `↓`, `tab` | Pick an account. With one account, `↑` `↓` scroll instead. |
| `space`, `a` | Mark one account, or all of them |
| `/` | Filter the accounts by name |
| `g` | Show every account as a card |
| `d` | Show more of the selected account |
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

`esc` goes back to the log. The side panel labels each list with its number, and clicking a list there opens it. With the side panel hidden (`B`), the divider shows every view as a tab instead.

The log sorts lines into channels: `mira`, `script`, `server`, `chat` and `game`. The divider lists them, with the one shown highlighted. `[` and `]` move one channel left or right, and stop at the ends. `main`, the default, shows everything except chat and game events. `all` shows every channel. Clicking a channel shows it.

In the drops list, `↑` `↓` move, `space` marks a drop, `a` marks them all, and `enter` accepts the marked drops, or the one under the cursor.

### Script args

Before a run starts, mira asks in the footer for each arg the script declares that you didn't type. It shows the type it expects and rejects an answer that doesn't fit. An optional arg says so, and shows its default when the script writes it as a plain value. `enter` on an empty answer skips it, so the script uses its default. With several accounts marked, it asks each account in turn, so each can get its own answer. Press `tab` before the first answer to give one answer for all of them; mira keeps that choice for the next run. mira remembers your answers until you quit and offers them the next time. Typing replaces an offered answer, and `backspace` edits it.

mira keeps the scripts you run in its data folder (`~/Library/Application Support/mira/history` on macOS), one entry per script with the args you last typed for it, and `↑` walks back through them, newest first, in later sessions. After them come the scripts your accounts are running, even ones started with `mira ctl` or from another terminal. Answers to asked args stay out of that file.

## Sessions from the shell

Each config gets its own session, so accounts from different `mira.toml` files can run at the same time.

| Command | Does |
| --- | --- |
| `mira up [ACCOUNT...] [-c PATH]` | Start the session in the background without opening the TUI |
| `mira serve [ACCOUNT...] [-c PATH] [--json \| --quiet]` | Run the session in the foreground and print its log, for systemd. `ctrl+c` logs everyone out; a second one exits at once. |
| `mira ps [--json]` | List every running session's accounts: state, script, where, uptime and status |
| `mira logs [ACCOUNT] [-f] [--all]` | Print an account's recent log, or every account's when only one session runs. `-f` keeps printing, and `--all` adds game lines. |
| `mira ctl ACCOUNT login\|logout\|stop\|restart\|pause\|resume\|relogin` | Drive an account in whichever session has it |
| `mira ctl ACCOUNT run SCRIPT [--arg KEY=VALUE]...` | Run a script, stopping the one running. Logs the account in if it's offline. |
| `mira down [-c PATH]` | Log out every account and end the session |

`ACCOUNT` is a name or a username. Sessions keep their sockets, locks and logs in mira's data folder (`~/Library/Application Support/mira/sessions` on macOS), which only you can open. Set `MIRA_SESSIONS` to use another folder. When `mira` attaches to a session that was already running, the TUI says when that session started, and warns if `mira.toml` changed after it. When the running session is from another build of mira, `mira` asks whether to restart it with this one, and restarts it without asking when the two builds can't talk to each other. A restart logs the online accounts out and back in, and their scripts start again.

## Other tools

| Command | Does |
| --- | --- |
| `mira check SCRIPT...` | Compile scripts and the files they import, without logging in or running them |
| `mira run -a ACCOUNT [SCRIPT] [--arg KEY=VALUE]...` | Log one account from `mira.toml` in, run its script or the one named, and log out when it ends |
| `mira run ... --record FILE` | Also write every packet to `FILE` as JSON lines, with the token and personal fields redacted |
| `mira view FILE [--cmd NAME]... [--full]` | Print a recording one packet per line, only the named commands if any, cut at 160 characters unless `--full` |
