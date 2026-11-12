# Configuration

`aqx.toml` lists your accounts and what each one runs. Each config has its own session. See the [CLI reference](cli.md) and [web UI](web.md) for session controls.

## aqx.toml

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

aqx looks for `./aqx.toml`, then `~/aqx/aqx.toml`. Set `AQX_HOME` to replace `~/aqx`, or pass `-c PATH` to select a file. The file holds passwords. aqx warns when other users can read it.

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

Script paths in the web UI also resolve relative to `aqx.toml`. New accounts appear offline within 2 seconds. Changes to existing accounts take effect after a session restart.
