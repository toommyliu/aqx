# `aqx.toml` reference

`aqx.toml` lists the accounts aqx logs in and the settings for each one. `aqx init` writes a starting file. `aqx schema` prints the JSON Schema.

## Where aqx looks

aqx uses the first file it finds:

1. The file given with `-c FILE`.
2. `aqx.toml` in the current folder.
3. `$AQX_HOME/aqx.toml`. Without `AQX_HOME`, `~/aqx/aqx.toml`.

aqx prints a warning when users other than you can read the file. To fix the warning, run `chmod 600 aqx.toml`.

## Example

```toml
#:schema ./aqx.schema.json

[defaults]
server = "Twilly"
on_stall = "relogin"

[[account]]
user = "your-username"
password = "your-password"
script = "farm.ts"

[[account]]
name = "alt"
user = "your-other-username"
password = "your-other-password"
script = "follower.ts"
args = { target = "your-username" }
```

## Shared settings

These settings go in `[defaults]`, in an `[[account]]`, or in both. A value in an `[[account]]` overrides the value in `[defaults]`.

| Setting | Value | Without it |
| --- | --- | --- |
| `server` | The game server to join, such as `"Twilly"`. `aqx servers` lists them. | The first server that is online, not full, and open to the account |
| `api` | The host that takes the login | `https://game.aq.com` |
| `room` | The room number that `map.join` uses when the map name has no room, such as `12345` | The server picks the room |
| `stall_after` | How long a script can go without progress before aqx marks the account stalled. A number with `ms`, `s`, `m`, or `h`, such as `"90s"`. A number without a unit is seconds. | `"5m"` |
| `on_stall` | What aqx does when the account stalls: `warn` logs a warning, `restart` runs the script again, and `relogin` logs out, logs back in, and runs the script again | `warn` |
| `restart` | `on-error` runs the script again after a delay when the script throws. `never` leaves the account idle. | `never` |
| `autologin` | `false` keeps the account offline when aqx starts a session without account names. Log it in later from the terminal UI or with `aqx ctl NAME login`. Naming the account, as in `aqx NAME`, still logs it in. | `true` |

## Account settings

These settings go only in an `[[account]]`.

| Setting | Value | Without it |
| --- | --- | --- |
| `user` | The account username | Required |
| `password` | The account password | Required |
| `name` | The name that `aqx ps`, `aqx ctl`, the terminal UI, and the logs show. Two accounts can't share a name. | `user` |
| `script` | The script to run after login. A relative path starts from the folder that holds `aqx.toml`. | The account logs in and waits for a command from `aqx ctl` or the terminal UI. |
| `args` | Values the script reads with `script.args()`: strings, numbers, or booleans. `--arg key=value` on the command line overrides a value. | No values |
