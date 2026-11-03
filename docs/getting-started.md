# Get started

In this tutorial, we install aqx, add one account, and run the sample script. The script joins `farm`, accepts quest 11, farms one Treeant Branch, and turns the quest in.

## Install the build tools

aqx runs on macOS and Linux. There are no prebuilt binaries, so we build aqx from source. Building needs a C compiler and Rust 1.89 or later.

1. Install a C compiler:

   | System | Command |
   | --- | --- |
   | macOS | `xcode-select --install` |
   | Debian, Ubuntu | `sudo apt install build-essential curl` |
   | Fedora | `sudo dnf install gcc` |

2. Install Rust with [rustup](https://rustup.rs):

   ```
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   If you already have Rust, run `rustup update` instead.

3. Open a new terminal, and run `cargo --version`. The version is 1.89 or later.

## Install aqx

1. Build and install aqx. The first build takes a few minutes.

   ```
   cargo install --locked --git https://github.com/toommyliu/aqx aqx
   ```

2. Run `aqx --version`. You see `aqx 0.1.0`.

## Make a script folder

1. Run `aqx init`.

2. aqx asks `Add an account to a new aqx.toml? [y/N]`. Type `y`.

3. Type your username, then your password. The password doesn't show as you type.

aqx creates the folder `~/aqx` and lists each file it writes:

```
wrote /Users/you/aqx/aqx.d.ts
wrote /Users/you/aqx/tsconfig.json
wrote /Users/you/aqx/.gitignore
wrote /Users/you/aqx/farm.ts
wrote /Users/you/aqx/aqx.toml
wrote /Users/you/aqx/aqx.schema.json
```

`farm.ts` is the sample script. `aqx.toml` holds your account and tells aqx to run `farm.ts` after login:

```toml
#:schema ./aqx.schema.json

[[account]]
user = "your-username"
password = "your-password"
script = "farm.ts"
```

`aqx.toml` holds your password, so only your user can read it. The `.gitignore` keeps it out of git.

## Check the script

Compile the script without logging in:

```
cd ~/aqx
aqx check farm.ts
```

You see `farm.ts: ok`. If the script has a mistake, aqx prints the line and column instead.

## Run the script

Run `aqx run`.

aqx logs in, runs `farm.ts`, and prints one line for each thing the account does. Look for `logged in`, then `joined farm-` with a room number, then a `killed` line for each monster. When the script ends, aqx logs out and exits. To stop early, press Ctrl-C.

## Next steps

- [How scripts run](scripts.md) covers what to know before you write your own script.
- The [`aqx.toml` reference](config.md) lists every setting, such as the server and what to do when a script stalls.
- `aqx --help` lists every command. Run `aqx` with no command to open the terminal UI with every account logged in.
- To upgrade aqx, run the `cargo install` command again. Then run `aqx init` to get the new `aqx.d.ts`.
