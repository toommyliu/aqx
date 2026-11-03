import * as native from "aqx:native";

export class AqxError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "AqxError";
    this.code = code;
  }
}

const handlers = new Map(
  ["death", "respawn", "map", "cell", "kill", "drop", "item", "quest", "chat", "warning", "aura", "counter"].map((name) => [name, []]),
);
const packetHandlers = new Map();

let delivering = 0;

native.init(
  (code, message) => new AqxError(code, message),
  (name, payload, seq) => {
    delivering = seq;
    const list = name === "packet" ? packetHandlers.get(payload.cmd) : handlers.get(name);
    for (const handler of [...(list ?? [])]) handler(payload);
  },
);

function subscribe(list, handler, first, last) {
  list.push(handler);
  if (list.length === 1) first();
  return () => {
    const at = list.indexOf(handler);
    if (at < 0) return;
    list.splice(at, 1);
    if (list.length === 0) last();
  };
}

const view = native.view;

function live(target, read, names) {
  for (const name of names) Object.defineProperty(target, name, { get: () => read()[name], enumerable: true });
  return target;
}

function byRef(ref) {
  if (typeof ref === "number") return (item) => item.id === ref;
  const name = String(ref).toLowerCase();
  return (item) => item.name.toLowerCase() === name;
}

function items(read, extra = {}) {
  const base = {
    get items() {
      return read();
    },
    get(ref) {
      return read().find(byRef(ref));
    },
    count(ref) {
      return read()
        .filter(byRef(ref))
        .reduce((n, item) => n + item.qty, 0);
    },
    has(ref, qty = 1) {
      return this.count(ref) >= qty;
    },
  };
  return Object.defineProperties(base, Object.getOwnPropertyDescriptors(extra));
}

const targetOf = (target) => (typeof target === "function" || Array.isArray(target) ? target : [target]);

function quietly(what, action) {
  try {
    action();
    return true;
  } catch (e) {
    if (!(e instanceof AqxError) || e.code === "cancelled") throw e;
    console.warn(`${what}: ${e.message}`);
    return false;
  }
}

const config = { room: native.room() ?? null, respawn: null, antiCounter: true };

export const settings = {
  get room() {
    return config.room;
  },
  set room(room) {
    config.room = room;
  },
  get respawn() {
    return config.respawn;
  },
  set respawn(spot) {
    native.respawn(spot?.cell, spot?.pad);
    config.respawn = spot ?? null;
  },
  get antiCounter() {
    return config.antiCounter;
  },
  set antiCounter(on) {
    config.antiCounter = Boolean(on);
  },
};

function fightOf(opts) {
  return {
    skills: (opts.skills ?? [1, 2, 3, 4]).map((step) => (typeof step === "number" ? { slot: step } : step)),
    strict: opts.strict ?? false,
    hold: opts.hold,
    antiCounter: config.antiCounter,
    cell: opts.cell,
    pad: opts.pad,
  };
}

export const player = live(
  { rest: () => native.rest() },
  () => view("player"),
  [
    "id",
    "name",
    "level",
    "hp",
    "maxHp",
    "mp",
    "maxMp",
    "state",
    "cell",
    "pad",
    "x",
    "y",
    "afk",
    "target",
    "gold",
    "coins",
    "member",
    "class",
    "stats",
    "auras",
    "factions",
  ],
);

export const map = live(
  {
    find: (target) => native.find(targetOf(target)),
    join(name, cell, pad = "Left") {
      native.join(name.includes("-") || config.room == null ? name : `${name}-${config.room}`);
      if (cell) native.jump(cell, pad);
    },
    jump: (cell, pad = "Left") => native.jump(cell, pad),
    walk: (x, y) => native.walk(x, y),
    goto: (name) => native.goto(String(name)),
    collect: (id, item, qty = 1) => native.collect(id, item, qty),
  },
  () => view("map"),
  ["name", "room", "areaId", "monsters", "players"],
);

export const combat = {
  aggro: (target) => native.aggro(target == null ? null : targetOf(target)),
  exit: () => native.exit(),
  kill(target, opts = {}) {
    native.kill(targetOf(target), { ...fightOf(opts), until: opts.until, timeout: opts.timeout });
  },
  farm(item, qty, opts) {
    const [goals, o = {}] = Array.isArray(item) ? [item, qty] : [[{ item, qty }], opts];
    const here = o.target === undefined && o.cell === undefined ? { cell: player.cell, pad: player.pad } : {};
    native.farm(
      goals.map((goal) => [goal.item, goal.qty]),
      targetOf(o.target ?? "*"),
      { ...fightOf({ ...o, ...here }), timeout: o.timeout },
    );
  },
};

export const skills = {
  get slots() {
    return view("skills");
  },
  use: (slot, target) => native.use_skill(slot, target == null ? null : targetOf(target)),
};

export const inventory = items(() => view("inventory"), {
  temp: items(() => view("temp")),
  get slots() {
    return view("slots").bag;
  },
  get used() {
    return view("slots").bagUsed;
  },
  equip: (item) => native.equip(item),
  use: (item) => native.use_item(item),
});

export const bank = items(() => native.bank(), {
  get slots() {
    return view("slots").bank;
  },
  get used() {
    return view("slots").bankUsed;
  },
  withdraw(...refs) {
    for (const ref of refs) native.withdraw(ref);
  },
  deposit(...refs) {
    for (const ref of refs) native.deposit(ref);
  },
});

export const events = {
  on(name, handler) {
    const list = handlers.get(name);
    if (!list) throw new TypeError(`unknown event ${name}; the events are ${[...handlers.keys()].join(", ")}`);
    return subscribe(
      list,
      handler,
      () => native.listen(name, true),
      () => native.listen(name, false),
    );
  },
  once(name, opts = {}) {
    let seen;
    const after = native.events_queued();
    const off = events.on(name, (e) => {
      if (!seen && delivering >= after && (!opts.filter || opts.filter(e))) seen = { e };
    });
    try {
      if (!native.wait_until(() => seen !== undefined, opts.timeout)) {
        throw new AqxError("timeout", `no ${name} event within ${opts.timeout} ms`);
      }
    } finally {
      off();
    }
    return seen.e;
  },
};

const registeredQuests = new Map();

function tendQuests() {
  for (const [id, reward] of registeredQuests) {
    const ok = quietly(`quest ${id}`, () => {
      if (native.quest(id).status === "completable") native.complete(id, reward, 1);
      if (native.quest(id).status === "available") native.accept(id);
    });
    if (!ok) {
      registeredQuests.delete(id);
      console.warn(`unregistered quest ${id}`);
    }
  }
}

events.on("item", () => registeredQuests.size && tendQuests());

export const quests = {
  get: (id) => native.quest(id),
  accept(...ids) {
    for (const id of ids) native.accept(id);
  },
  complete: (id, opts = {}) => native.complete(id, opts.reward, opts.times ?? 1),
  get registered() {
    return [...registeredQuests.keys()];
  },
  register(...entries) {
    for (const entry of entries) {
      const [id, reward] = typeof entry === "number" ? [entry, undefined] : [entry.id, entry.reward];
      native.quest(id);
      registeredQuests.set(id, reward);
    }
    tendQuests();
  },
  unregister(...ids) {
    for (const id of ids) registeredQuests.delete(id);
  },
};

export const shop = {
  get: (id) => native.shop(id),
  buy: (id, item, qty = 1) => native.buy(id, item, qty),
  sell: (item, qty = 1) => native.sell(item, qty),
};

const dropRules = [];
const wantedDrop = (drop) => dropRules.some((rule) => (typeof rule === "function" ? rule(drop) : byRef(rule)(drop)));

function collectDrops() {
  for (const drop of view("drops")) {
    if (wantedDrop(drop)) quietly(`picking up ${drop.name}`, () => native.pickup(drop.id));
  }
}

events.on("drop", () => dropRules.length && collectDrops());

export const drops = {
  get pending() {
    return view("drops");
  },
  pickup(...refs) {
    for (const ref of refs) native.pickup(ref);
  },
  get registered() {
    return [...dropRules];
  },
  register(...rules) {
    dropRules.push(...rules);
    collectDrops();
  },
  unregister(...rules) {
    for (const rule of rules) {
      const same = typeof rule === "string" ? (r) => typeof r === "string" && r.toLowerCase() === rule.toLowerCase() : (r) => r === rule;
      for (let i = dropRules.length - 1; i >= 0; i--) if (same(dropRules[i])) dropRules.splice(i, 1);
    }
  },
};

export const packet = {
  send: (text) => native.send_packet(String(text)),
  on(cmd, handler) {
    cmd = String(cmd);
    if (!packetHandlers.has(cmd)) packetHandlers.set(cmd, []);
    return subscribe(
      packetHandlers.get(cmd),
      handler,
      () => native.listen_packet(cmd, true),
      () => native.listen_packet(cmd, false),
    );
  },
  request(text, cmd, opts = {}) {
    const timeout = opts.timeout ?? 10_000;
    let seen;
    const after = native.events_queued();
    const off = packet.on(cmd, (p) => {
      if (!seen && delivering >= after && (!opts.filter || opts.filter(p))) seen = { p };
    });
    try {
      native.send_packet(String(text));
      if (!native.wait_until(() => seen !== undefined, timeout)) throw new AqxError("timeout", `no ${cmd} reply within ${timeout} ms`);
    } finally {
      off();
    }
    return seen.p;
  },
};

export const session = {
  get server() {
    return native.server();
  },
  servers: () => native.servers(),
  relogin: (server) => native.relogin(server == null ? undefined : String(server)),
};

export const wait = {
  until: (predicate, timeout) => native.wait_until(predicate, timeout),
  ms: (ms) => native.sleep(ms),
};

const kind = (fallback) => typeof (typeof fallback === "function" ? fallback() : fallback);

function parseArg(key, raw, kind) {
  if (kind === "number") {
    const n = Number(raw);
    if (raw.trim() === "" || !Number.isFinite(n)) throw new TypeError(`--arg ${key}=${raw} isn't a number`);
    return n;
  }
  if (kind === "boolean") {
    if (raw !== "true" && raw !== "false") throw new TypeError(`--arg ${key}=${raw} isn't true or false`);
    return raw === "true";
  }
  return raw;
}

export const script = {
  args(defaults) {
    const out = { ...defaults };
    const given = native.args(defaults);
    for (const [key, raw] of Object.entries(given)) {
      if (!Object.hasOwn(defaults, key)) {
        throw new TypeError(`unknown --arg ${key}; this script takes ${Object.keys(defaults).join(", ") || "no args"}`);
      }
      out[key] = parseArg(key, raw, kind(defaults[key]));
    }
    const missing = Object.keys(defaults).filter((key) => typeof defaults[key] === "function" && !Object.hasOwn(given, key));
    if (missing.length > 0) throw new TypeError(`missing ${missing.map((key) => `--arg ${key}`).join(", ")}`);
    return out;
  },
  status: (text) => native.status(String(text)),
  progress: (label, current, total) => native.progress(String(label), current, total),
  stop: (reason) => native.stop(reason == null ? undefined : String(reason)),
};
