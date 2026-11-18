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

[[account.schedule]]
at = "9pm"
script = "dailies.ts"

[[account]]
name = "healer"
user = "player2"
password = "change-me"
server = "Yorumi"
auto_start = false
```

A session ends when every account it logged in goes offline. Put `keep_alive = true` at the top of the file, above `[defaults]`, to keep it running until `aqx down`. A session with [schedules](#schedules) also keeps running.

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
| `resume` | After `aqx down`, `ctrl+c` or a crash, the next session brings back the script and queue the account had. See [Resume](#resume). Defaults to `false`. |
| `stall_after` | How long a running script can go without progress before the account counts as stalled, like `"90s"`, `"5m"` or `"1h"`. Progress is a kill, an item change, a quest turn-in or a map join. Defaults to `"5m"`. |
| `on_stall` | What a stall does: `warn` logs a warning, `restart` restarts the script, and `relogin` logs out, back in, and starts the script again. Defaults to `warn`. |

## Schedules

An account's `schedule` list runs scripts at set times. Each `[[account.schedule]]` runs one script at a time of day.

| Key | Meaning |
| --- | --- |
| `at` | Time of day, like `"21:00"`, `"9pm"` or `"9:30pm"`. Required. |
| `days` | Days to run on, like `["fri"]` or `["saturday", "sunday"]`. Every day unless set. |
| `tz` | Time zone for `at`, like `"America/New_York"`. Defaults to this machine's time zone. |
| `relogin` | Log out and back in first. Defaults to `false`. |
| `script` | Script to run, relative to `aqx.toml`. Required unless `relogin` is on. |
| `args` | Arguments for that script. |

When a schedule comes due:

- A running script stops and goes to the front of the queue, so it starts again once the scheduled script ends.
- An online account with no script runs the scheduled script, then stays idle.
- An offline account logs in, runs the scheduled script, then logs out. Its queue waits for the next login.
- A paused script keeps running, and aqx logs that it skipped the schedule.
- A schedule that comes due while another scheduled script runs waits behind it.
- With `relogin = true`, an online account logs out and back in before anything else. A schedule with no script then starts the running script again, or leaves an idle account idle. An offline account skips a relogin schedule that has no script.

A scheduled script that throws doesn't restart, even with `restart_on_error` on. aqx logs the error and moves on to the next script. A manual stop holds the queue as usual.

Schedules fire only while a session runs, and a session with schedules keeps running when every account goes offline. A schedule never runs late. One missed while the session was down doesn't run, and one that comes due more than a minute late, like when the computer sleeps through it, is skipped and logged. AQW's daily quests reset at midnight US Eastern time, so `at = "00:00"` with `tz = "America/New_York"` runs right after the reset.

## Skua updates

A `[skua]` section sets what a session does with the [Skua script library](skua.md) when it starts:

```toml
[skua]
update = "check"
```

| `update` | When a session starts |
| --- | --- |
| `off` | Nothing. The default. |
| `check` | aqx asks GitHub whether the library changed, without downloading it. If it did, the web UI shows a toast with an Update button. |
| `auto` | aqx downloads the library if it changed upstream, and the web UI says so in a toast. |

Accounts log in while the check runs, so a script that starts before a download finishes keeps the old library until it stops and uses the new one on its next start. A toast stays until you dismiss it, and the next session's check can show a new one. A failed check shows its error the same way.

## Resume

With `resume = true`, aqx saves the account's running script and queue whenever they change. If the session ends with `aqx down`, `ctrl+c` or a crash, the next session starts the account the way the last one left it, in place of `auto_start` and `script`:

- An account that was running a script logs in and runs that script again from the start.
- An account that was online with nothing running logs in and stays idle.
- An offline account stays offline. Its queue comes back either way.

A session that ends because every account logged out leaves nothing to resume, so the next one follows `auto_start` again. Schedules aren't saved. An account resumes the script a schedule interrupted, and an account that a schedule logged in resumes offline. Accounts named with `aqx up ACCOUNT` start as asked and get their queues back, and the other saved accounts resume too.

Script paths in the web UI also resolve relative to `aqx.toml`. New accounts appear offline within 2 seconds. Changes to existing accounts take effect after a session restart.
