import { combat, settings, map, script, type Script } from "mira";

export default function* (): Script {
  // stop skills that would attack the monster
  settings.counterHold = "attacks";

  if ((yield* map.join("doomvaultb", { cell: "r26" })) === "refused") {
    script.log("couldnt join map");
    script.stop();
  }

  yield* combat.kill("undead raxgore");
}
