# Run accounts from the browser

`aqx web` serves the web UI at `http://127.0.0.1:4970`.

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

`-c` selects the config. `aqx web` attaches to its session if one is running. Otherwise, logging in or running a script starts a session. `--up` starts the session immediately and logs in accounts with `auto_start` enabled. Stopping `aqx web` leaves the session running. The session ends when every account goes offline, but `aqx web` stays open so you can start it again. The Session menu can restart the session or log everyone out.

`aqx web` listens on `127.0.0.1` only and has no login. It refuses requests whose `Host` isn't `localhost`, `127.0.0.1`, `[::1]` or a `*.ts.net` name.

## Reach it from another device

`--tailscale` puts it on your tailnet with [Tailscale Serve](https://tailscale.com/kb/1312/serve), over HTTPS on the same port:

```sh
aqx web --tailscale
```

This requires `tailscale` on your `PATH` and prints a URL for devices on your tailnet. The route persists after `aqx web` stops. If a route to aqx already exists, the command prints it instead of adding another. If the port serves something else, choose another `--port`. To remove the default route, run `tailscale serve --https=4970 --set-path=/ off`. Anyone who can reach the address can control your accounts, so keep it off [Funnel](https://tailscale.com/kb/1223/funnel).

## Edit aqx.toml

Saved passwords are hidden. An empty password field keeps the saved password.

aqx rejects invalid changes and refuses to save if the file changed on disk since the editor opened. See the [configuration reference](config.md) for the keys and when changes take effect.

## Keyboard shortcuts

Press `?` to view and change shortcuts. aqx saves them in its data folder, so every browser uses the same shortcuts.
