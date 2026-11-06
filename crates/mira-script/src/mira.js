import * as native from "mira:native";

const STEP = Symbol("step");

export class MiraError extends Error {
  constructor(message) {
    super(message);
    this.name = "MiraError";
  }
}

const eventHandlers = new Map();
const packetHandlers = new Map();
let nextTag = 1;

function started(entries, payload) {
  const actions = [];
  for (const { tag, handler } of [...entries]) {
    const result = handler(payload);
    if (isAction(result)) actions.push([tag, delegate(result)]);
  }
  return actions;
}

native.init(
  STEP,
  MiraError,
  (name, payload) => started(eventHandlers.get(name) ?? [], payload),
  (kind, cmd, packet) => started(packetHandlers.get(`${kind}:${cmd}`) ?? [], packet),
);

function listen(table, key, handler) {
  const tag = nextTag++;
  const entries = table.get(key) ?? [];
  entries.push({ tag, handler });
  table.set(key, entries);
  return tag;
}

function unlisten(table, key, handler) {
  const entries = table.get(key) ?? [];
  const at = entries.findIndex((entry) => entry.handler === handler);
  if (at < 0) return;
  const [{ tag }] = entries.splice(at, 1);
  if (entries.length === 0) table.delete(key);
  native.halt(tag);
}

function spawn(task) {
  const tag = nextTag++;
  native.spawn(tag, delegate(task));
  return tag;
}

const unstarted = new Map();

function runnable(run) {
  const value = {
    *[Symbol.iterator]() {
      unstarted.delete(value);
      return yield* run();
    },
  };
  if (unstarted.size === 0) Promise.resolve().then(reportUnstarted);
  unstarted.set(value, new Error().stack);
  return value;
}

function reportUnstarted() {
  for (const stack of unstarted.values()) {
    const caller = stack
      .split("\n")
      .map((line) => /\((.+)\)$/.exec(line.trim())?.[1])
      .find((at) => at !== undefined && !at.startsWith("mira") && at !== "native");
    native.warn(`an action${caller === undefined ? "" : ` at ${caller}`} never ran; run it with yield*`);
  }
  unstarted.clear();
}

function action(op) {
  return runnable(function* () {
    return yield { [STEP]: op() };
  });
}

function act(name, args = {}) {
  return action(() => ({ kind: "act", name, ...args }));
}

function text(value, what) {
  if (typeof value !== "string") throw new TypeError(`${what} must be a string`);
  return value;
}

function id(value, what) {
  if (typeof value !== "number") throw new TypeError(`${what} must be a number`);
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function coordinate(value, axis) {
  if (typeof value !== "number") throw new TypeError(`map.walk's ${axis} must be a number`);
  if (!Number.isInteger(value)) throw new RangeError(`map.walk can't walk to ${axis} ${value}`);
  return value;
}

function reader(read, missing) {
  let seen = -1;
  let value;
  return () => {
    const version = native.version();
    if (version !== seen) {
      value = read();
      seen = version;
    }
    if (value === null) throw new MiraError(missing);
    return value;
  };
}

function view(target, read, names) {
  for (const name of names) {
    Object.defineProperty(target, name, { get: () => read()[name], enumerable: true });
  }
  return Object.freeze(target);
}

const me = reader(() => native.view("player"), "the character has not loaded yet");
const room = reader(() => native.view("map"), "the player is not in a map yet");

export const player = view(
  {
    rest() {
      return act("rest");
    },
  },
  me,
  [
    "entityId", "charId", "name", "level", "exp", "maxExp", "hp", "maxHp", "mp", "maxMp",
    "state", "away", "gold", "coins", "member", "class", "classRank", "stats", "auras",
    "factions", "boosts",
  ],
);

export const map = view(
  {
    join(name, options = {}) {
      const map = text(name, "map.join's map name");
      const cell = options.cell === undefined ? undefined : text(options.cell, "map.join's cell");
      const pad = options.pad === undefined ? undefined : text(options.pad, "map.join's pad");
      if (pad !== undefined && cell === undefined) throw new TypeError("map.join's pad needs a cell");
      return act("join", { map, cell, pad });
    },
    walk(x, y) {
      return act("walk", { x: coordinate(x, "x"), y: coordinate(y, "y") });
    },
    goto(name) {
      return act("goto", { player: text(name, "map.goto's player") });
    },
    collect(mapItemId) {
      return act("collect", { item: id(mapItemId, "map.collect's mapItemId") });
    },
    setSpawnPoint(cell, pad = "Left") {
      if (!native.setSpawnPoint(text(cell, "map.setSpawnPoint's cell"), text(pad, "map.setSpawnPoint's pad"))) {
        throw new MiraError("the session isn't running yet");
      }
    },
    jump(cell, pad) {
      return act("jump", { cell: text(cell, "map.jump's cell"), pad: pad === undefined ? undefined : text(pad, "map.jump's pad") });
    },
  },
  room,
  ["name", "areaId", "cell", "pad", "x", "y", "zone", "players", "monsters"],
);

function monsterRef(value, what) {
  if (typeof value === "string") return value;
  if (typeof value !== "number") throw new TypeError(`${what} must be a monMapId or a monster name`);
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function playerRef(value, what) {
  if (typeof value === "string") return value;
  if (typeof value !== "number") throw new TypeError(`${what} must be an entityId or a player name`);
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

const fighting = reader(() => native.view("combat"), "the character has not loaded yet");

function alive(monster) {
  return monster.state !== "dead" && monster.hp > 0;
}

function matching(ref) {
  const { cell, monsters } = room();
  return monsters.filter(
    (monster) =>
      monster.cell === cell &&
      (typeof ref === "number" ? monster.monMapId === ref : monster.name.toLowerCase() === ref.toLowerCase()),
  );
}

function slots(value) {
  if (!Array.isArray(value)) throw new TypeError("combat.kill's skills must be an array of slots");
  if (value.length === 0) throw new RangeError("combat.kill needs at least one skill");
  for (const slot of value) {
    if (typeof slot !== "number") throw new TypeError("combat.kill's skills must be slot numbers");
    if (!Number.isInteger(slot) || slot < 1 || slot > 5) throw new RangeError(`combat.kill can't use slot ${slot}`);
  }
  return [...value];
}

export const combat = view(
  {
    kill(target, options = {}) {
      const ref = monsterRef(target, "combat.kill's target");
      const skills = options.skills === undefined ? undefined : slots(options.skills);
      const kill = () => act("kill", { target: ref, skills });
      if (!options.waitForRespawn) return kill();
      return runnable(function* () {
        const settled = () => matching(ref).length === 0 || matching(ref).some(alive);
        if (!settled()) yield* wait.until(settled);
        if (matching(ref).length === 0) return "none";
        return yield* kill();
      });
    },
    attack(target) {
      return act("attack", { target: monsterRef(target, "combat.attack's target") });
    },
    useSkill(slot, target) {
      if (typeof slot !== "number") throw new TypeError("combat.useSkill's slot must be a number");
      if (!Number.isInteger(slot) || slot < 1 || slot > 5) throw new RangeError(`combat.useSkill can't use slot ${slot}`);
      if (target === undefined) return act("skill", { slot });
      if (target === null || typeof target !== "object") throw new TypeError("combat.useSkill's target must be { monster } or { player }");
      if ((target.monster === undefined) === (target.player === undefined)) {
        throw new TypeError("combat.useSkill's target names either a monster or a player");
      }
      return target.monster !== undefined
        ? act("skill", { slot, monster: monsterRef(target.monster, "combat.useSkill's monster") })
        : act("skill", { slot, player: playerRef(target.player, "combat.useSkill's player") });
    },
    exit() {
      return act("exit");
    },
    untarget() {
      if (!native.untarget()) throw new MiraError("the session isn't running yet");
    },
  },
  fighting,
  ["skills", "target"],
);

function itemRef(value, what) {
  if (typeof value === "string") return value;
  if (typeof value !== "number") throw new TypeError(`${what} must be an item id or an item name`);
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function count(value, what) {
  if (typeof value !== "number") throw new TypeError(`${what} must be a number`);
  if (!Number.isInteger(value) || value < 1) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function find(items, ref) {
  if (typeof ref === "number") return items.find((item) => item.id === ref);
  const name = ref.toLowerCase();
  return items.find((item) => item.name.toLowerCase() === name);
}

const notLoaded = "the character has not loaded yet";
const bag = reader(() => native.view("inventory"), notLoaded);
const vault = reader(() => native.view("bank"), notLoaded);
const offers = reader(() => native.view("drops"), notLoaded);
const store = reader(() => native.view("shop"), notLoaded);

export const inventory = view(
  {
    get(item) {
      return find(bag().items, itemRef(item, "inventory.get's item"));
    },
    count(item) {
      const ref = itemRef(item, "inventory.count's item");
      return (find(bag().items, ref) ?? find(bag().temp, ref))?.qty ?? 0;
    },
    canFit(drop) {
      if (drop === null || typeof drop !== "object") throw new TypeError("inventory.canFit takes a drop");
      return native.hasRoom(id(drop.id, "inventory.canFit's drop id"), text(drop.pool, "inventory.canFit's drop pool"));
    },
    equip(item) {
      return act("equip", { item: itemRef(item, "inventory.equip's item") });
    },
    unequip(item) {
      return act("unequip", { item: itemRef(item, "inventory.unequip's item") });
    },
    use(item) {
      return act("use", { item: itemRef(item, "inventory.use's item") });
    },
  },
  bag,
  ["items", "temp", "bagSlots", "houseSlots"],
);

export const bank = view(
  {
    load() {
      return act("loadBank");
    },
    deposit(item) {
      return act("deposit", { item: itemRef(item, "bank.deposit's item") });
    },
    withdraw(item) {
      return act("withdraw", { item: itemRef(item, "bank.withdraw's item") });
    },
    swap(inventoryItem, bankItem) {
      return act("swap", {
        item: itemRef(inventoryItem, "bank.swap's inventory item"),
        bankItem: itemRef(bankItem, "bank.swap's bank item"),
      });
    },
  },
  vault,
  ["items", "slots"],
);

const wantedDrops = [];
let dropsWatched = false;

function wanted(offer) {
  return wantedDrops.some((ref) =>
    typeof ref === "number" ? offer.id === ref : offer.name.toLowerCase() === ref.toLowerCase(),
  );
}

function* pickupLogged(offer) {
  try {
    if ((yield* drops.pickup(offer.id)) === "no_room") script.warn(`no room to pick up ${offer.name}`);
  } catch (e) {
    if (!(e instanceof MiraError)) throw e;
    script.warn(`couldn't pick up ${offer.name}: ${e.message}`);
  }
}

export const drops = view(
  {
    pickup(item) {
      return act("pickup", { item: itemRef(item, "drops.pickup's item") });
    },
    get registered() {
      return [...wantedDrops];
    },
    register(...items) {
      const refs = items.map((item) => itemRef(item, "drops.register's item"));
      for (const ref of refs) if (!wantedDrops.includes(ref)) wantedDrops.push(ref);
      if (!dropsWatched) {
        dropsWatched = true;
        listen(eventHandlers, "dropOffered", (offer) => (wanted(offer) ? pickupLogged(offer) : undefined));
      }
      for (const offer of offers().pending) if (wanted(offer)) spawn(pickupLogged(offer));
    },
    unregister(...items) {
      for (const item of items) {
        const at = wantedDrops.indexOf(itemRef(item, "drops.unregister's item"));
        if (at >= 0) wantedDrops.splice(at, 1);
      }
    },
  },
  offers,
  ["pending"],
);

const questLoops = new Map();

function* questLoop(registration) {
  const { id: questId, reward } = registration;
  try {
    if (quests.get(questId) === undefined) yield* quests.load(questId);
    while (true) {
      if (!quests.get(questId)?.accepted) yield* quests.accept(questId);
      yield* wait.until(() => {
        const quest = quests.get(questId);
        return quest === undefined || !quest.accepted || quest.turnIns > 0;
      });
      if ((quests.get(questId)?.turnIns ?? 0) > 0) {
        yield* quests.complete(questId, reward === undefined ? {} : { reward });
      }
    }
  } catch (e) {
    script.warn(`quest ${questId} failed: ${e instanceof Error ? e.message : e}`);
    questLoops.delete(questId);
  }
}

function registration(value) {
  if (typeof value === "number") return { id: id(value, "quests.register's id") };
  if (value === null || typeof value !== "object") throw new TypeError("quests.register takes quest ids or { id, reward }");
  const registered = { id: id(value.id, "quests.register's id") };
  if (value.reward !== undefined) registered.reward = itemRef(value.reward, "quests.register's reward");
  return Object.freeze(registered);
}

export const quests = Object.freeze({
  get registered() {
    return [...questLoops.values()].map((loop) => loop.registration);
  },
  register(...registrations) {
    for (const registered of registrations.map(registration)) {
      const running = questLoops.get(registered.id);
      if (running) native.halt(running.tag);
      questLoops.set(registered.id, { registration: registered, tag: spawn(questLoop(registered)) });
    }
  },
  unregister(...ids) {
    for (const questId of ids.map((value) => id(value, "quests.unregister's id"))) {
      const running = questLoops.get(questId);
      if (!running) continue;
      questLoops.delete(questId);
      native.halt(running.tag);
    }
  },
  get(questId) {
    const quest = native.quest(id(questId, "quests.get's id"));
    return quest === null ? undefined : quest;
  },
  load(...ids) {
    if (ids.length === 0) throw new TypeError("quests.load needs at least one quest id");
    return act("loadQuests", { ids: ids.map((questId) => id(questId, "quests.load's id")) });
  },
  accept(questId) {
    return act("accept", { id: id(questId, "quests.accept's id") });
  },
  complete(questId, options = {}) {
    const reward = options.reward === undefined ? undefined : itemRef(options.reward, "quests.complete's reward");
    const times = options.times === undefined ? 1 : count(options.times, "quests.complete's times");
    return act("complete", { id: id(questId, "quests.complete's id"), reward, times });
  },
});

export const shop = view(
  {
    load(shopId) {
      return act("loadShop", { id: id(shopId, "shop.load's id") });
    },
    buy(item, qty) {
      const units = qty === undefined ? undefined : count(qty, "shop.buy's qty");
      if (item !== null && typeof item === "object") {
        return act("buy", { listingId: id(item.listingId, "shop.buy's listingId"), qty: units });
      }
      return act("buy", { item: itemRef(item, "shop.buy's item"), qty: units });
    },
    sell(item, qty) {
      const units = qty === undefined ? 1 : count(qty, "shop.sell's qty");
      return act("sell", { item: itemRef(item, "shop.sell's item"), qty: units });
    },
  },
  store,
  ["current"],
);

function isAction(value) {
  return typeof value?.[Symbol.iterator] === "function";
}

function delegate(action) {
  unstarted.delete(action);
  return (function* () {
    return yield* action;
  })();
}

export const wait = {
  until(predicate) {
    if (typeof predicate !== "function") throw new TypeError("wait.until takes a function");
    return action(() => ({ kind: "until", predicate }));
  },
  ms(ms) {
    if (typeof ms !== "number") throw new TypeError("wait.ms takes a number of milliseconds");
    if (!(ms >= 0 && ms < Infinity)) throw new RangeError(`wait.ms can't wait ${ms} ms`);
    return action(() => ({ kind: "ms", ms }));
  },
  first(actions) {
    if (actions === null || typeof actions !== "object") throw new TypeError("wait.first takes an object of actions");
    const entries = Object.entries(actions);
    if (entries.length === 0) throw new TypeError("wait.first needs at least one action");
    for (const [key, value] of entries) {
      if (!isAction(value)) throw new TypeError(`wait.first: ${key} is not an action`);
    }
    for (const [, value] of entries) unstarted.delete(value);
    return action(() => ({ kind: "first", entries: entries.map(([key, value]) => [key, delegate(value)]) }));
  },
};

const EVENTS = new Set([
  "joined", "moved", "playerEntered", "playerLeft", "playerMoved", "zone", "chat", "whisper",
  "notice", "killed", "died", "respawned", "leveledUp", "dropOffered", "itemChanged",
  "questAccepted", "questCompleted", "auraAdded", "auraRemoved", "counterStarted",
  "counterEnded", "awayChanged",
]);

function eventName(name, what) {
  if (!EVENTS.has(name)) throw new TypeError(`${what}: ${String(name)} isn't an event`);
  return name;
}

function handlerOf(handler, what) {
  if (typeof handler !== "function") throw new TypeError(`${what} takes a function`);
  return handler;
}

function filterOf(filter, what) {
  if (filter !== undefined && typeof filter !== "function") throw new TypeError(`${what}'s filter must be a function`);
  return filter;
}

export const events = Object.freeze({
  on(name, handler) {
    listen(eventHandlers, eventName(name, "events.on"), handlerOf(handler, "events.on"));
  },
  off(name, handler) {
    unlisten(eventHandlers, eventName(name, "events.off"), handler);
  },
  once(name, filter) {
    const event = eventName(name, "events.once");
    return action(() => ({ kind: "once", event, filter: filterOf(filter, "events.once") }));
  },
});

function packetKey(match, what) {
  if (match === null || typeof match !== "object") throw new TypeError(`${what} takes { kind, cmd }`);
  if (match.kind !== "json" && match.kind !== "xt") throw new TypeError(`${what}'s kind must be "json" or "xt"`);
  return { kind: match.kind, cmd: text(match.cmd, `${what}'s cmd`) };
}

function watchedPackets() {
  let count = 0;
  for (const entries of packetHandlers.values()) count += entries.length;
  native.watchPackets(count);
}

export const packets = Object.freeze({
  on(match, handler) {
    const { kind, cmd } = packetKey(match, "packets.on");
    listen(packetHandlers, `${kind}:${cmd}`, handlerOf(handler, "packets.on"));
    watchedPackets();
  },
  off(match, handler) {
    const { kind, cmd } = packetKey(match, "packets.off");
    unlisten(packetHandlers, `${kind}:${cmd}`, handler);
    watchedPackets();
  },
  once(match, filter) {
    const { kind, cmd } = packetKey(match, "packets.once");
    return action(() => ({ kind: "packet", packetKind: kind, cmd, filter: filterOf(filter, "packets.once") }));
  },
  send(packet) {
    return act("send", { text: text(packet, "packets.send's packet") });
  },
});

const COUNTER_HOLDS = ["off", "all", "attacks"];
const config = { room: null, antiAfk: false, counterHold: "off", aggro: "off", autoRelogin: false };
let aggroWatched = false;

function applyAggro() {
  const aggro = config.aggro;
  if (aggro === "off" || aggro === "cell") return native.setAggro(aggro, []);
  const ids = new Set();
  for (const monster of room().monsters) {
    if (aggro.some((ref) => (typeof ref === "number" ? monster.monMapId === ref : monster.name.toLowerCase() === ref.toLowerCase()))) {
      ids.add(monster.monMapId);
    }
  }
  for (const ref of aggro) if (typeof ref === "number") ids.add(ref);
  return native.setAggro("only", [...ids]);
}

function session(done) {
  if (!done) throw new MiraError("the session isn't running yet");
}

export const settings = {
  get room() {
    return config.room;
  },
  set room(value) {
    if (value !== null) id(value, "settings.room");
    session(native.setRoom(value));
    config.room = value;
  },
  get antiAfk() {
    return config.antiAfk;
  },
  set antiAfk(on) {
    if (typeof on !== "boolean") throw new TypeError("settings.antiAfk must be true or false");
    session(native.setAntiAfk(on));
    config.antiAfk = on;
  },
  get counterHold() {
    return config.counterHold;
  },
  set counterHold(hold) {
    if (!COUNTER_HOLDS.includes(hold)) throw new TypeError(`settings.counterHold must be one of ${COUNTER_HOLDS.join(", ")}`);
    session(native.setCounterHold(hold));
    config.counterHold = hold;
  },
  get aggro() {
    return config.aggro;
  },
  set aggro(aggro) {
    if (aggro !== "off" && aggro !== "cell") {
      if (!Array.isArray(aggro)) throw new TypeError('settings.aggro must be "off", "cell", or monsters');
      aggro = Object.freeze(aggro.map((ref) => monsterRef(ref, "settings.aggro's monster")));
    }
    config.aggro = aggro;
    session(applyAggro() !== false);
    if (Array.isArray(aggro) && !aggroWatched) {
      aggroWatched = true;
      const reapply = () => {
        if (Array.isArray(config.aggro)) applyAggro();
      };
      listen(eventHandlers, "joined", reapply);
      listen(eventHandlers, "moved", reapply);
    }
  },
  get autoRelogin() {
    return config.autoRelogin;
  },
  set autoRelogin(policy) {
    if (policy === false) {
      native.setAutoRelogin(null, 0, null);
      config.autoRelogin = false;
      return;
    }
    if (policy === null || typeof policy !== "object") throw new TypeError("settings.autoRelogin must be false or { delayMs }");
    if (typeof policy.delayMs !== "number") throw new TypeError("settings.autoRelogin's delayMs must be a number");
    if (!(policy.delayMs >= 0 && policy.delayMs < Infinity)) throw new RangeError(`settings.autoRelogin can't wait ${policy.delayMs} ms`);
    const attempts = policy.attempts === undefined ? 5 : count(policy.attempts, "settings.autoRelogin's attempts");
    const server = policy.server === undefined ? null : text(policy.server, "settings.autoRelogin's server");
    native.setAutoRelogin(policy.delayMs, attempts, server);
    config.autoRelogin = Object.freeze({ ...policy });
  },
};
Object.seal(settings);

export const auth = Object.freeze({
  get server() {
    return native.server();
  },
  servers() {
    return action(() => ({ kind: "servers" }));
  },
  relogin(server) {
    const name = server === undefined ? undefined : text(server, "auth.relogin's server");
    return action(() => ({ kind: "relogin", server: name }));
  },
  logout() {
    return action(() => ({ kind: "logout" }));
  },
});

function show(value) {
  if (typeof value === "string") return value;
  if (value instanceof Error || typeof value !== "object" || value === null) return String(value);
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

const say = (emit) => (...args) => emit(args.map(show).join(" "));

export const script = {
  log: say(native.log),
  warn: say(native.warn),
  error: say(native.error),
  status(text) {
    native.status(String(text));
  },
  stop(reason) {
    const text = reason === undefined ? undefined : String(reason);
    native.stop(text);
    throw new MiraError(text ?? "the script stopped");
  },
  restart() {
    native.restart();
    throw new MiraError("the script is restarting");
  },
  args(defaults = {}) {
    if (defaults === null || typeof defaults !== "object") throw new TypeError("script.args takes an object of defaults");
    const given = native.args();
    if (given === undefined) throw new MiraError("script.args can't run before the script starts");
    return Object.freeze({ ...defaults, ...given });
  },
};
