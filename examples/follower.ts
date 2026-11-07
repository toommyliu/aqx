import { combat, inventory, map, script, wait, type Script } from "mira";

export default function* (): Script {
  const { target, class: className } = script.args<{ target: string; class?: string }>({
    class: "",
  });

  let shown = "";
  const status = (text: string) => {
    if (text !== shown) {
      shown = text;
      script.status(text);
    }
  };

  if (className) {
    const owned = inventory.get(className);
    if (!owned) script.stop(`no ${className} in the inventory`);
    if (!owned.equipped) {
      status(`equipping ${owned.name}`);
      yield* inventory.equip(owned.id);
    }
  }

  const name = target.toLowerCase();
  const together = () =>
    map.players.some((p) => p.name.toLowerCase() === name && p.cell === map.cell);
  const mon = () =>
    map.monsters.find((m) => m.cell === map.cell && m.state !== "dead");

  while (true) {
    if (!together()) {
      status(`going to ${target}`);
      let went = yield* map.goto(target);
      while (went !== "arrived") {
        status(`can't reach ${target} (${went}); trying every 5 s`);
        yield* wait.ms(5000);
        went = yield* map.goto(target);
      }
      continue;
    }

    status(`with ${target} in ${map.cell}`);
    const monster = mon();
    if (!monster) {
      yield* wait.until(() => !together() || mon() !== undefined);
      continue;
    }
    yield* wait.first({
      kill: combat.kill(monster.monMapId),
      left: wait.until(() => !together()),
    });
  }
}
