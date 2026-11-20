import assert from "node:assert/strict";
import test from "node:test";
import { farmProgress, latestFarmGoal } from "../src/farming.ts";

const line = (channel, text) => ({ channel, text });

test("a Skua farming goal survives unrelated lines and ends with its run", () => {
  const message = "(_KillForItem) Farming Aeacus Empowered (0/100)";
  const goal = latestFarmGoal([line("script", `[12:34:56] ${message}`)], null);
  assert.deepEqual(goal, { item: "Aeacus Empowered", have: 0, need: 100 });
  const hunt = `(HuntMonster) Killing Underfiend for item: "Traitor's Tract" 0/250`;
  assert.deepEqual(latestFarmGoal([line("script", hunt)], goal), { item: "Traitor's Tract", have: 0, need: 250 });
  assert.deepEqual(latestFarmGoal([line("script", `repeated 2 more times: [12:34:56] ${message}`)], null), goal);
  assert.deepEqual(latestFarmGoal([
    line("chat", "(KillMonster) Farming Something Else (3/5)"),
    line("script", "(KillMonster) Farming Something Else (3/0)"),
    line("script", "status: moving"),
  ], goal), goal);
  assert.equal(latestFarmGoal([line("script", message), line("aqx", "restarting the script")], goal), null);
  const next = latestFarmGoal([
    line("aqx", "running Farm.cs"),
    line("script", "(KillMonster) Farming ⚔️ Rune (Bright) (2/5)"),
  ], goal);
  assert.deepEqual(next, { item: "Rune (Bright)", have: 2, need: 5 });
  assert.equal(latestFarmGoal([line("aqx", "the script finished")], next), null);
});

test("Skua item goals use the count and target from the corresponding log format", () => {
  for (const [message, item, have, need] of [
    ['(HuntMonsterMulti) Farming "Bone Dust" 2/10', "Bone Dust", 2, 10],
    ['(Farm) Farming "Dark Spirit Orb" 3 / 50', "Dark Spirit Orb", 3, 50],
    ["(KillVath) ⚡ Killing Vath for Ore (Bright) (2/10) [Temp = True]", "Ore (Bright)", 2, 10],
    ["(KillUltraBoss) Killing Ultra-Boss Beast for Rune (5) [Temp = False]", "Rune", 0, 5],
    ['(_MonsterHunt) Hunting "Treeant" for "Treeant Branch" x5', "Treeant Branch", 0, 5],
    ["(ScriptMain) Hunting For: Vegetable Token, 5/300", "Vegetable Token", 5, 300],
    ["(BuyItem) Farming Dragon Runestone x12", "Dragon Runestone", 0, 12],
    ["(FarmSunlightTo) Farming Sliver of Sunlight (need 20, have 4)...", "Sliver of Sunlight", 4, 20],
    ["(ScriptMain) [Phase 2] Farming Sliver of Moonlight in /solsticemoon until inventory ≥ 25.", "Sliver of Moonlight", 0, 25],
    ["(BuyScrolls) Getting you 10x Scroll of Enrage", "Scroll of Enrage", 0, 10],
    ["(ScriptMain) Progress: 2/4 Condensed Grace", "Condensed Grace", 2, 4],
    ["(Farm) Farming Mourning Flower (2/10) and Jus Divinum Scale (4/8)", "Mourning Flower", 2, 10],
    ["(Indulgence) Farming 75 Souls of Limbo...", "Souls of Limbo", 0, 75],
    ["(SaeculumGem) Farming 40 Saeculum Gem", "Saeculum Gem", 0, 40],
  ]) {
    assert.deepEqual(latestFarmGoal([line("script", `[12:34:56] ${message}`)], null), { item, have, need }, message);
  }
});

test("bare counters require a known item before they replace the farming goal", () => {
  const account = {
    registered: { drops: ["Bone Dust"] },
    game: { inventory: [{ name: "Tainted Gem" }], temp: [], drops: [{ name: "Scroll of Enrage" }] },
  };
  const parse = (text) => latestFarmGoal([line("script", `(Farm) ${text}`)], null, account);
  assert.deepEqual(parse("Tainted Gem: 2/50"), { item: "Tainted Gem", have: 2, need: 50 });
  assert.deepEqual(parse("Scroll of Enrage x3/8"), { item: "Scroll of Enrage", have: 3, need: 8 });
  assert.deepEqual(parse("Farming 30 Bone Dust..."), { item: "Bone Dust", have: 0, need: 30 });
  for (const message of [
    "Boss kill signal: 1/3", "Progress: 4/8", "Farming to buy Armor (#1/3)",
    "Farming 500 gold using HonorHall Method", "🔥 Killing Ultra Xiang for (none) (0/1) [Temp = False]",
  ]) {
    assert.equal(parse(message), null, message);
  }
});

test("farming counts held stacks by name or item ID", () => {
  const stack = (qty) => ({ id: 21, name: "Aeacus Empowered", qty });
  const game = { inventory: [stack(12)], temp: [stack(3)], drops: [stack(25)], bank: [stack(40)] };
  for (const item of ["aeacus empowered", "21"]) {
    const goal = { item, have: 4, need: 100 };
    assert.deepEqual(farmProgress(goal, null), goal);
    assert.deepEqual(farmProgress(goal, game), { item: "Aeacus Empowered", have: 15, need: 100 });
    assert.equal(farmProgress(goal, { ...game, inventory: [], temp: [] }).have, 0);
  }
});
