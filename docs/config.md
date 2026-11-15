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

## Schedules

A `[[schedule]]` runs scripts on an account at a time of day:

```toml
[[schedule]]
name = "morning farm"
account = "player1"
script = "farm/void-aura.ts"
args = { target = 7500 }
at = "09:30"
days = ["weekdays"]
until = "12:00"

[[schedule]]
account = "healer"
scripts = [
  { script = "daily/quests.ts" },
  { script = "daily/bank.ts", args = { keep = 10 } },
]
at = "21:00"
for = "1h"
timezone = "America/New_York"
when_busy = "interrupt"
after = "resume"
```

| Key | Meaning |
| --- | --- |
| `account` | The account to run on, by `name` or `user`. Required. |
| `script`, `args` | One script to run, relative to `aqx.toml`, and its arguments, as on an account. |
| `scripts` | Several scripts to run one after another, each `{ script = "...", args = { ... } }`. Use instead of `script` and `args`. |
| `at` | Start time, 24-hour `"HH:MM"`. Required. |
| `days` | Days to run: `"mon"`, `"tue"`, `"wed"`, `"thu"`, `"fri"`, `"sat"`, `"sun"`, `"weekdays"` or `"weekends"`. Defaults to every day. |
| `until` | Stop time, `"HH:MM"`. A time earlier than `at` is the next day. |
| `for` | How long to run, like `"2h"`, instead of `until`. |
| `timezone` | An IANA time zone for `at`, `until` and `days`, like `"America/New_York"`. Defaults to the machine's time zone. |
| `when_busy` | What happens when the account is already running a script. `skip` skips this run and logs it. `stop` stops that script and drops it. `interrupt` stops it and puts it first in the queue, where it starts again from the beginning. It doesn't carry on from where it stopped. Defaults to `skip`. |
| `after` | What happens when the run ends. `stay` stays logged in, and the queue goes on only if the last script finished. `resume` goes on with the queue however the run ended, while the account is online. `logout` logs the account out. Defaults to `stay`. |
| `catch_up` | When the session starts, wakes from sleep, or loads the schedule inside its window, start now and stop at `until`, or at `at` plus `for`. Needs `until` or `for`. Defaults to `false`. |
| `enabled` | Whether the schedule runs. Defaults to `true`. |
| `name` | What logs call the schedule. Defaults to the first script's name and the start time, like `"void-aura at 09:30"`. Names are unique, ignoring case. |

Schedules run only while the config's session runs. A session whose `aqx.toml` has an enabled schedule stays up while every account is offline, waiting for the next one, until `aqx down` or the web UI logs everyone out. Without `catch_up`, a schedule fires only within a minute after `at`, so a session that starts later waits for the next day. If the machine was asleep at `at`, or the session starts inside the window of a schedule with `until` or `for`, the account's log says the schedule was missed.

A schedule logs the account in if it's offline, the same way `aqx ctl ACCOUNT run` does. If the login gives up, the run ends there and the account stays offline. Logging in kicks the same account out of anywhere else it's playing, such as the game client or another config's session, so a schedule can do that while you aren't watching. With `scripts`, the later scripts wait at the front of the account's queue and each starts when the one before it finishes. When the run stops early, at `until` or `for`, or after an error, the scripts it hadn't started leave the queue. A run you take over by running, stopping or logging out the account, or with `aqx ctl ACCOUNT next`, is yours from then on: `until`, `for` and `after` no longer apply. A relogin or restart keeps it under the schedule. A session restart doesn't: the script carries on as an ordinary run, and the scripts it hadn't started stay queued.

A time that daylight saving skips runs when the clock jumps past it. A time that happens twice runs once, the first time. `until` follows the clock on the wall, and `for` counts real time. Every start, skip, miss, stop and logout appears in the account's log under `aqx`. Schedule changes take effect within 2 seconds, without a session restart. A run that already started keeps the settings it started with.
