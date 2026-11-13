import assert from "node:assert/strict";
import test from "node:test";
import { createRoot, onCleanup } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { follow } from "../src/wire.ts";

const document = Object.assign(new EventTarget(), { visibilityState: "visible" });
let source;
Object.assign(globalThis, {
  document,
  matchMedia: () => ({ matches: false }),
  IntersectionObserver: class {
    constructor(changed) { this.changed = changed; }
    observe(target) { this.changed([{ target, isIntersecting: true }]); }
    unobserve() {}
    disconnect() {}
  },
  EventSource: class extends EventTarget {
    static CLOSED = 2;
    readyState = 1;
    constructor() { super(); source = this; }
    close() { this.readyState = 2; }
  },
});
const { createCountdown } = await import("../src/timing.ts");

function account() {
  const aura = () => ({ name: "Timed", stacks: 1, description: "", timer: { duration_ms: 10000, left_ms: 10000 } });
  return {
    name: "player1", user: "player1", state: "Running", server: "Test", script: null, status: "",
    registered: null, up: { secs: 60, nanos: 0 }, stats: { kills: 0, deaths: 0, drops: 0, gold_from: 0 },
    gold_gained: 0, recording: null, queue: [],
    game: {
      area: "farm-1", cell: "Enter", pad: "Spawn", hp: 1000, max_hp: 1000, mp: 100, max_mp: 100,
      dead: false, away: false, member: false, class: "Mage", class_rank: 10, level: 100, exp: 0, exp_to_level: 100, gold: 100,
      skills: [{ name: "Strike", ready: 0.25, left_ms: 10000 }], auras: [aura()],
      target: { id: 1, name: "Frogzard", hp: 100, max_hp: 500, auras: [aura()] },
      players: [], drops: [], quests: [], factions: [], temp: [], inventory: [], bag_used: 0, misc_used: 0,
      bag_slots: 50, bank: null, bank_used: 0, bank_slots: 0,
    },
  };
}

for (const event of ["accounts", "account"]) {
  test(`equal-valued ${event} samples refresh player, target and skill deadlines`, (t) => {
    let now = 0;
    t.mock.method(performance, "now", () => now);
    const emit = (name, data) => source.dispatchEvent(new MessageEvent(name, { data: JSON.stringify(data) }));
    let dispose;
    const timers = createRoot((stop) => {
      dispose = stop;
      const [accounts, setAccounts] = createStore([]);
      onCleanup(follow(null, {
        session() {}, log() {}, ended() {}, connected() {},
        accounts: (all) => setAccounts(reconcile(all)),
        account: (at, next) => setAccounts(at, reconcile(next)),
      }));
      emit("accounts", [account()]);
      const sample = () => accounts[0];
      const auraTimer = (aura) => createCountdown(() => ({
        progress: 1 - aura().timer.left_ms / aura().timer.duration_ms,
        leftMs: aura().timer.left_ms, running: true, sampledAt: sample().sampledAt,
      }));
      const timers = [
        auraTimer(() => sample().game.auras[0]),
        auraTimer(() => sample().game.target.auras[0]),
        createCountdown(() => ({
          progress: sample().game.skills[0].ready, leftMs: sample().game.skills[0].left_ms,
          running: true, sampledAt: sample().sampledAt,
        })),
      ];
      for (const timer of timers) timer.observe({});
      return timers;
    });
    try {
      const seconds = () => timers.map((timer) => timer.seconds());
      assert.deepEqual(seconds(), [10, 10, 10]);
      now = 5000;
      document.dispatchEvent(new Event("visibilitychange"));
      assert.deepEqual(seconds(), [5, 5, 5]);
      emit(event, event === "accounts" ? [account()] : { i: 0, account: account() });
      assert.deepEqual(seconds(), [10, 10, 10], "a refresh rebases even when all wire values match");
      now = 10000;
      document.dispatchEvent(new Event("visibilitychange"));
      assert.deepEqual(seconds(), [5, 5, 5], "refreshed timers outlive the original deadline");
      now = 15000;
      document.dispatchEvent(new Event("visibilitychange"));
      assert.deepEqual(seconds(), [0, 0, 0]);
    } finally {
      dispose();
    }
  });
}
