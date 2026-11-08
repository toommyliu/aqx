# Write a mira script

A mira script is a TypeScript file whose default export is a generator function. mira logs in, runs that function, and logs out when it returns. [`api/mira.d.ts`](../api/mira.d.ts) lists everything a script can call.

```ts
import { combat, map, script, type Script } from "mira";

export default function* (): Script {
	const { monster, kills } = script.args<{ monster: string; kills?: number }>({ kills: 10 });
	yield* map.join("greenguardwest");
	for (let i = 0; i < kills; i++) {
		if ((yield* combat.kill(monster, { waitForRespawn: true })) === "died") {
			script.stop("the player died");
		}
	}
}
```

```sh
mira run farm.ts --user player1 --pass "$MIRA_PASS" --server Twilly --arg monster=Frogzard
```

## Set up type checking

Copy [`examples/tsconfig.json`](../examples/tsconfig.json) next to your scripts, and point its `paths` entry for `"mira"` at `api/mira.d.ts`. mira removes types without compiling, so it rejects syntax that generates code, such as `enum`. The config's `erasableSyntaxOnly` option makes your editor flag that syntax too.

## Put the script in the default export

mira runs the module's top-level code when it loads the script, before it logs in. Top-level code has no game state, so reading `player.hp` or calling `script.args()` there throws. Keep imports, constants, and helper functions at the top level, and put everything that runs in the game inside the default export.

The run ends when the default function returns. Event handlers and anything else running alongside it stop at that point.

## Run actions with `yield*`

An action is anything whose return type is `Action<T>`, such as `map.join` or `wait.ms`. Run one with `yield*`. The expression evaluates to the action's result.

```ts
if ((yield* map.join("battleon")) === "refused") script.log("battleon is full");
```

`yield` without the star throws a `TypeError`, and an action that never runs logs a warning with its location. To catch the missing star in the editor, give the default export and your own generator functions the `Script` return type.

To reuse a sequence of actions, write a generator function and call it with `yield*`:

```ts
function* restUntilFull(): Script {
	while (player.hp < player.maxHp) yield* player.rest();
}
```

Actions run only inside generator functions, so callbacks such as `.forEach` can't run them. Use a `for...of` loop instead.

## Read state between actions

Fields such as `player.hp` and `map.monsters` read the current state and send nothing to the server. State changes only at a `yield*`, so two reads with no `yield*` between them return the same value.

`map.cells` holds the map's frame labels. mira downloads them once a map finishes loading and keeps them for the rest of the session, so the list is empty for a moment after joining a map for the first time.

To wait for a condition, use `wait.until`. mira checks the predicate right away and again whenever state changes. A predicate can't run actions.

```ts
yield* wait.until(() => inventory.count("Dark Gem") >= 50);
```

## Handle failures

An action returns the outcomes you should expect, such as `"refused"` or `"died"`. Anything else throws a `MiraError`. A bad argument throws a `TypeError` or `RangeError` before mira sends anything.

An error that nothing catches ends the run, and mira prints it with the script's stack trace.

## Run actions together

`wait.first` runs several actions at once. When one finishes, mira stops the rest and tells you which one won.

```ts
const outcome = yield* wait.first({
	kill: combat.kill("Escherion"),
	timeout: wait.ms(60_000),
});
if (outcome.winner === "timeout") yield* combat.exit();
```

An action starts only when something runs it, so pass actions to `wait.first` without `yield*`.

An event handler that is a generator function also runs alongside the default function. It can run actions without blocking the main loop:

```ts
events.on("killed", function* () {
	if (player.mp < 20) yield* inventory.use("Mana Potion");
});
```

## Clean up in `finally` blocks

When the script stops, mira runs its `finally` blocks. That happens on `script.stop()`, on Ctrl+C, when the connection drops, and when an action loses a `wait.first`. A `catch` block can't keep the script going after a stop.

Don't run actions in a `finally` block. mira doesn't wait for them, and the rest of the block after the first `yield*` doesn't run.

## Take arguments

`script.args` returns the values passed with `--arg key=value`. mira reads the type from the source, so write it inline, and import `script` by name rather than with `import * as`:

```ts
const { monster, kills } = script.args<{ monster: string; kills?: number }>({ kills: 10 });
```

Optional keys take their value from the defaults you pass. Lists and records are JSON on the command line. mira checks the values before it logs in.

## Split a script across files

A script can import `"mira"` and relative paths with the extension written out, such as `"./lib/rest.ts"`. It can't import packages.

## Run the script

```sh
mira run <script> --user <user> --pass <pass> --server <server> [--arg key=value]...
```

`mira servers` lists the server names. Press Ctrl+C once to stop the script and log out, or twice to exit right away.

In the TUI, `S` lists the `.ts`, `.js` and `.mjs` files in the `scripts` folder next to `mira.toml`, `~/mira/scripts` by default. It skips `.d.ts` files, `node_modules`, and folders whose names start with a dot. Press `/` to search by name and folder, and `enter` to run the selected script. mira then asks for each arg the script declares.
