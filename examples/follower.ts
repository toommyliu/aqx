import { AqxError, combat, inventory, map, player, script, wait } from "aqx";

const args = script.args({ target: "", class: "" });
if (!args.target) script.stop("pass --arg target=<player name>");
if (args.class) inventory.equip(args.class);

const name = args.target.toLowerCase();
const together = () => map.players.find((p) => p.name.toLowerCase() === name)?.cell === player.cell;

while (true) {
  if (!together()) {
    script.status(`going to ${args.target}`);
    try {
      map.goto(args.target);
    } catch (e) {
      if (!(e instanceof AqxError) || e.code !== "timeout") throw e;
      script.status(`can't reach ${args.target}, retrying`);
      wait.ms(5000);
      continue;
    }
    wait.until(together, 1000);
    continue;
  }
  script.status(`fighting with ${args.target} in ${player.cell}`);
  combat.kill("*", { cell: player.cell, pad: player.pad, until: () => !together() });
}
