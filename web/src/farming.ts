import type { Account, Game, Line, Stack } from "./wire";

export type FarmGoal = { item: string; have: number; need: number };

const skuaLine = /^(?:repeated \d+ more times?: )?(?:\[\d{2}:\d{2}:\d{2}\] )?\([^()\r\n]+\) (.+)$/u;
const goalFormats = [
  /^Farming (?:⚔️ |💎 )?(?<item>.+?) \((?<have>\d+)\/(?<need>\d+)\)(?: and .+ \(\d+\/\d+\))?$/u,
  /^(?:Killing .+? for item: |Farming )"(?<item>.+)" (?<have>\d+) *\/ *(?<need>\d+)$/u,
  /^(?:⚡ |🌀 |🔥 )?Killing .+? for (?<item>.+?) \((?:(?<have>\d+)\/)?(?<need>\d+)\) \[Temp = (?:True|False)\]$/u,
  /^Hunting ".+" for "(?<item>.+)" x(?<need>\d+)$/u,
  /^Hunting For: (?<item>.+), (?<have>\d+)\/(?<need>\d+)$/u,
  /^Farming (?<item>.+) x(?<need>\d+)$/u,
  /^Farming (?<item>.+) \(need (?<need>\d+), have (?<have>\d+)\)\.\.\.$/u,
  /^\[Phase \d+\] Farming (?<item>.+) in \/\w+ until inventory ≥ (?<need>\d+)\.$/u,
  /^Getting you (?<need>\d+)x (?<item>.+)$/u,
  /^Progress: (?<have>\d+)\/(?<need>\d+) (?<item>.+)$/u,
  /^(?<label>.+?)(?:: | x)(?<have>\d+) *\/ *(?<need>\d+)$/u,
  /^Farming (?<need>\d+) (?:(?<item>Souls of Limbo|Saeculum Gem)|(?<label>.+?))(?:\.\.\.)?$/u,
];

function knownItem(label: string, account?: Account): boolean {
  const name = label.toLowerCase();
  const matches = (stack: Stack) => stack.name.toLowerCase() === name;
  const game = account?.game;
  return !!(account?.registered?.drops.some((drop) => typeof drop === "string" && drop.toLowerCase() === name)
    || game?.inventory.some(matches) || game?.temp.some(matches) || game?.drops.some(matches));
}

function parseFarmGoal(text: string, account?: Account): FarmGoal | null {
  const message = skuaLine.exec(text)?.[1];
  if (!message) return null;
  for (const format of goalFormats) {
    const fields = format.exec(message)?.groups;
    if (!fields) continue;
    const item = (fields.item ?? fields.label)?.trim();
    if (!item || item === "(none)" || (fields.label && !knownItem(item, account))) return null;
    const have = Number(fields.have ?? 0);
    const need = Number(fields.need);
    if (Number.isSafeInteger(have) && Number.isSafeInteger(need) && need > 0) return { item, have, need };
  }
  return null;
}

export function latestFarmGoal(lines: Line[], previous: FarmGoal | null, account?: Account): FarmGoal | null {
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i];
    if (!line) continue;
    if (line.channel === "aqx" && /^(?:running |restarting the script\b|the script (?:finished|stopped)\b)/.test(line.text)) {
      return null;
    }
    if (line.channel !== "script") continue;
    const goal = parseFarmGoal(line.text, account);
    if (goal) return goal;
  }
  return previous;
}

export function farmProgress(goal: FarmGoal, game: Game | null): FarmGoal {
  if (!game) return goal;
  const name = goal.item.toLowerCase();
  const id = /^\d+$/.test(name) ? Number(name) : null;
  const matches = (stack: Stack) => stack.id === id || stack.name.toLowerCase() === name;
  const bag = game.inventory.find(matches);
  const temp = game.temp.find(matches);
  return { item: bag?.name ?? temp?.name ?? goal.item, have: (bag?.qty ?? 0) + (temp?.qty ?? 0), need: goal.need };
}
