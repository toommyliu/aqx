# CLI reference

## Sessions

Each config gets its own session, so accounts from different `aqx.toml` files can run at the same time.

| Command | Does |
| --- | --- |
| `aqx up [ACCOUNT...] [-c PATH] [--json \| --quiet]` | Run the session in this terminal and print its log. `ctrl+c` logs everyone out; a second one exits at once. Fails when the session is already running. |
| `aqx up -d [ACCOUNT...] [-c PATH]` | Start the session in the background and return. When it's already running, log in the named accounts. |
| `aqx up [-d] [ACCOUNT...] --no-script` | Log the accounts in without running their scripts |
| `aqx ps [--json]` | List every running session's accounts: state, script and how many are queued, where, uptime and status |
| `aqx logs [ACCOUNT] [-f] [--all]` | Print an account's recent log, or every account's when only one session runs. `-f` keeps printing, and `--all` adds game lines and the packets of a recording account. |
| `aqx ctl ACCOUNT login\|logout\|stop\|restart\|pause\|resume\|relogin` | Drive an account in whichever session has it |
| `aqx ctl ACCOUNT run SCRIPT [--arg KEY=VALUE]...` | Run a script, stopping the one running. Logs the account in if it's offline. |
| `aqx ctl ACCOUNT queue SCRIPT [--arg KEY=VALUE]...` | Add a script to the end of the account's queue. See [Queue scripts](#queue-scripts). |
| `aqx ctl ACCOUNT queue --clear` | Empty the account's queue |
| `aqx ctl ACCOUNT next` | Stop the script and run the first one in the queue. Logs the account in if it's offline. |
| `aqx ctl ACCOUNT record [FILE]` | Write the account's packets to `FILE` as JSON lines from now until `aqx ctl ACCOUNT record --stop`, with the token and personal fields redacted. The account has to be logged in to start, and the recording carries on through relogins and server switches. Without `FILE`, aqx writes to `recordings` in its data folder (`~/Library/Application Support/aqx/recordings` on macOS). |
| `aqx down [-c PATH]` | Log out every account and end the session |

`ACCOUNT` is a name or a username. Sessions store sockets, locks, and logs in aqx's data folder (`~/Library/Application Support/aqx/sessions` on macOS). Set `AQX_SESSIONS` to use another folder.

When `aqx up -d` attaches to a session from another build, it asks whether to restart the session. If the builds cannot communicate, it restarts the session without asking. A restart logs online accounts out and back in and restarts their scripts.

## Queue scripts

Each account has a script queue. The next script starts when the current script finishes or stops itself. Adding a script does not start it. For an idle account, `aqx ctl ACCOUNT next` starts the first queued script.

A manual stop, an error, or a logout holds the queue. With `restart_on_error` on, the current script restarts after an error and the queue advances when it finishes. A relogin, server switch, stall restart, or reconnect restarts the current script. Running a script by hand leaves the queue intact.

Queues persist across a session restart if at least one account is online to start the new session. `aqx down` clears them, as does a session ending when every account goes offline.

## Other tools

| Command | Does |
| --- | --- |
| `aqx check SCRIPT...` | Compile scripts and the files they import, without logging in or running them |
| `aqx skua update\|list [QUERY]\|path` | Download or update the Skua script library, search it, or print its folder. See [Run Skua scripts](skua.md). |
| `aqx run -a ACCOUNT [SCRIPT] [--arg KEY=VALUE]...` | Log one account from `aqx.toml` in, run its script or the one named, and log out when it ends |
| `aqx run ... --record FILE` | Also write every packet to `FILE` as JSON lines, with the token and personal fields redacted |
