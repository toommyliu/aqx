# How scripts run

A script is a `.ts` file that runs from top to bottom on one account. [`aqx.d.ts`](../api/aqx.d.ts) documents each call.

## Each call waits until it's done

aqx has no promises and no `await`. Each call returns when its work is finished:

- `map.join("farm")` returns when the player stands in the map.
- `quests.accept(11)` returns when the quest is active.
- `combat.farm("Treeant Branch", 5)` returns when the inventory holds 5.

To wait for anything else, use `wait.until(() => condition)`.

## Failed calls throw `AqxError`

Every `AqxError` has a `code`:

| `code` | Cause |
| --- | --- |
| `timeout` | The game didn't answer in time. |
| `rejected` | The server refused, such as a quest the account can't accept. |
| `not_found` | The item, monster, quest, or shop doesn't exist. |
| `cancelled` | The script is stopping, or the connection closed. |

To retry after a timeout, catch the error and check the code:

```ts
import { AqxError, map, wait } from "aqx";

try {
	map.goto("SomePlayer");
} catch (e) {
	if (!(e instanceof AqxError) || e.code !== "timeout") throw e;
	wait.ms(5000);
}
```

An error that the script doesn't catch ends the script. The error shows the line in your `.ts` file. To run the script again after an error, set `restart = "on-error"` in [`aqx.toml`](config.md).

## Background work runs while a call waits

While the script waits inside a call, aqx keeps fighting and runs the work you registered:

- `drops.register(...)` picks up matching drops.
- `quests.register(...)` accepts and completes quests each time an item changes.
- `events.on(...)` and `packet.on(...)` run their handlers.
- `setTimeout` and `setInterval` run their callbacks.

A loop that never calls aqx never gives them a turn.

A predicate is a function that aqx calls to test a condition, such as the `until` option of `combat.kill` or the function you pass to `wait.until`. A predicate can read state, but it can't call anything that waits. For example, `map.join` inside a predicate throws an error.

## Arguments come from three places

`script.args` declares the inputs of a script. The default value of each key sets its type:

```ts
const args = script.args({ target: String, class: "" });
```

`target: String` has no default, so the argument is required. `class: ""` defaults to an empty string.

The values come from these places. Each one overrides the one before:

1. The defaults in the script.
2. `args` for the account in `aqx.toml`.
3. `--arg key=value` on the command line.

Before the terminal UI runs a script, it asks for each argument that you didn't pass.

## A script's setup ends with the script

A script ends when it returns, throws, or calls `script.stop()`. It also ends when you stop it from the terminal UI or with `aqx ctl`. The account stays logged in. `aqx run` is the exception: it logs out and exits.

Everything the script set up ends with it. That includes handlers, timers, registered quests and drops, `combat.aggro`, and the status line and progress bars. `settings` goes back to the defaults for the account.

The game state stays, because the server keeps it. That includes the inventory, the bank, the current map, and accepted quests.

When you stop a script, the call it's waiting in throws `cancelled`. Catching that error doesn't keep the script running.

## Long loops must call aqx every 5 seconds

aqx reads the game connection only while the script is inside a call. If a script runs for 5 seconds without a call, aqx ends the script with an error. In a long loop, call something that waits, such as `wait.ms(100)`.

## aqx flags scripts that stop making progress

aqx watches each running script for progress. These events count as progress:

- A kill.
- An item change.
- A completed quest.
- A map join.
- A `script.progress` bar that moves forward.

`script.status` doesn't count. If nothing counts for 5 minutes, aqx marks the account stalled. `stall_after` and `on_stall` in [`aqx.toml`](config.md) change the time limit and what happens next.

A long step with no kills or loot can look stalled. Report it with `script.progress(label, current, total)`. The terminal UI then shows a bar with an ETA.

## Some TypeScript doesn't run

TypeScript features that generate code don't work: `enum`, namespaces that hold values, and constructor parameter properties. The `tsconfig.json` from `aqx init` makes your editor mark them.

A script can import `aqx` and other `.ts` files by relative path, such as `./lib/fight.ts`. npm packages and Node modules aren't available.
