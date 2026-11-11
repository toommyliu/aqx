# Set up aqx and run sessions

`aqx.toml` lists your accounts and what each one runs. Every config gets a session that keeps its accounts logged in. You start and stop sessions from the shell, and drive them from the shell or from the [web UI](web.md).

## Write aqx.toml

`aqx init` sets up `~/aqx`, or the folder you name. It writes `aqx.d.ts`, a `tsconfig.json` that points at it, a sample script, `aqx.schema.json` for editor completion, and a `.gitignore` that keeps `aqx.toml` out of git. In a terminal it also asks for one account and writes an `aqx.toml` that runs the sample. Running it again updates `aqx.d.ts` and the schema and leaves your other files alone.

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

Script paths you type in the web UI also resolve relative to `aqx.toml`. The session checks the file every 2 seconds and adds any `[[account]]` with a new name, offline. Edits to accounts that are already in the session, `auto_start` included, take effect when it restarts, and each changed account's log says so. Run `aqx down`, then `aqx up -d`, to apply them, or restart the session from the web UI's Session menu.

## Run sessions

Each config gets its own session, so accounts from different `aqx.toml` files can run at the same time.

| Command | Does |
| --- | --- |
| `aqx up [ACCOUNT...] [-c PATH] [--json \| --quiet]` | Run the session in this terminal and print its log. `ctrl+c` logs everyone out; a second one exits at once. Fails when the session is already running. |
| `aqx up -d [ACCOUNT...] [-c PATH]` | Start the session in the background and return. When it's already running, log in the named accounts. |
| `aqx up [-d] [ACCOUNT...] --no-script` | Log the accounts in without running their scripts |
| `aqx ps [--json]` | List every running session's accounts: state, script, where, uptime and status |
| `aqx logs [ACCOUNT] [-f] [--all]` | Print an account's recent log, or every account's when only one session runs. `-f` keeps printing, and `--all` adds game lines and the packets of a recording account. |
| `aqx ctl ACCOUNT login\|logout\|stop\|restart\|pause\|resume\|relogin` | Drive an account in whichever session has it |
| `aqx ctl ACCOUNT run SCRIPT [--arg KEY=VALUE]...` | Run a script, stopping the one running. Logs the account in if it's offline. |
| `aqx ctl ACCOUNT record [FILE]` | Write the account's packets to `FILE` as JSON lines from now until `aqx ctl ACCOUNT record --stop`, with the token and personal fields redacted. The account has to be logged in to start, and the recording carries on through relogins and server switches. Without `FILE`, aqx writes to `recordings` in its data folder (`~/Library/Application Support/aqx/recordings` on macOS). |
| `aqx down [-c PATH]` | Log out every account and end the session |

`ACCOUNT` is a name or a username. Sessions keep their sockets, locks and logs in aqx's data folder (`~/Library/Application Support/aqx/sessions` on macOS), which only you can open. Set `AQX_SESSIONS` to use another folder. When `aqx up -d` attaches to a session that was already running, it says when that session started, and warns if `aqx.toml` changed after it. When the running session is from another build of aqx, `aqx up -d` asks whether to restart it with this one, and restarts it without asking when the two builds can't talk to each other. A restart logs the online accounts out and back in, and their scripts start again.

## Other tools

| Command | Does |
| --- | --- |
| `aqx check SCRIPT...` | Compile scripts and the files they import, without logging in or running them |
| `aqx skua update\|list [QUERY]\|path` | Download or update the Skua script library, search it, or print its folder. See [Run Skua scripts](skua.md). |
| `aqx run -a ACCOUNT [SCRIPT] [--arg KEY=VALUE]...` | Log one account from `aqx.toml` in, run its script or the one named, and log out when it ends |
| `aqx run ... --record FILE` | Also write every packet to `FILE` as JSON lines, with the token and personal fields redacted |
