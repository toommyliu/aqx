/**
 * A script's default export is a generator function, and aqx runs it. Run an
 * action with `yield*`.
 *
 * An action throws an AqxError when it fails in a way its return type doesn't
 * name. An error that nothing catches ends the run.
 *
 * A number that can't apply, such as a negative count or a fractional
 * attempt, throws a RangeError before aqx sends anything.
 *
 * Handler calls run alongside the script. Predicates read state and can't run
 * actions.
 *
 * aqx stops a generator when it loses a `wait.first`, when its handler is
 * removed, and when the run ends. Stopping cancels its action and runs its
 * `finally` blocks.
 *
 * A run can hold 100 pending handler calls and 100 pending actions. Past
 * either limit, the run ends.
 *
 * A dropped connection ends the run. With `settings.autoRelogin` on, aqx logs
 * back in to the same server and starts the script again from the top.
 */

const STEP: unique symbol = Symbol("step");

interface Step {
  readonly [STEP]: unknown;
}

/** An action, or a call to your own generator function. */
export interface Action<T> {
  [Symbol.iterator](): Iterator<Step, T, unknown>;
}

export type ValueOf<X> = X extends Action<infer T> ? T : never;

/**
 * The return type for the default export and your own generator functions.
 * With it, `yield` without the star is a type error.
 */
export type Script<T = void> = Generator<Step, T, unknown>;

export type ItemRef = number | string;
/** A monMapId, or a name in the player's cell. */
export type MonsterRef = number | string;
/** An entityId, or a name in the map. */
export type PlayerRef = number | string;
/** 0 auto attack, 1-4 class skills, 5 the consumable slot. */
export type Slot = 0 | 1 | 2 | 3 | 4 | 5;
export type CombatState = "idle" | "combat" | "dead";

export interface Aura {
  readonly name: string;
  readonly stacks: number;
  readonly remainingMs: number | null;
}

export interface Stats {
  readonly strength: number;
  readonly endurance: number;
  readonly dexterity: number;
  readonly intellect: number;
  readonly wisdom: number;
  readonly luck: number;
  /** 0.25 is 25%. */
  readonly hit: number;
  readonly evasion: number;
  readonly crit: number;
  readonly haste: number;
}

export interface Faction {
  readonly id: number;
  readonly name: string;
  readonly rep: number;
  readonly rank: number;
}

export type SkillStatus =
  | { readonly kind: "ready" | "casting" | "disabled" | "locked" | "no_mana" }
  | { readonly kind: "cooling"; readonly readyInMs: number };

export interface Skill {
  readonly slot: Slot;
  readonly name: string;
  readonly mana: number;
  readonly cooldownMs: number;
  readonly target: "hostile" | "friendly" | "self";
  readonly maxTargets: number;
  readonly status: SkillStatus;
}

export interface Monster {
  readonly monMapId: number;
  readonly monId: number;
  readonly name: string;
  readonly cell: string;
  readonly hp: number;
  readonly maxHp: number;
  readonly state: CombatState;
  readonly auras: readonly Aura[];
}

export interface Player {
  /** The id raw packets use for this player. */
  readonly entityId: number;
  readonly name: string;
  readonly level: number | null;
  readonly class: string | null;
  readonly cell: string | null;
  readonly pad: string | null;
  readonly x: number | null;
  readonly y: number | null;
  readonly hp: number;
  readonly maxHp: number;
  readonly mp: number;
  readonly maxMp: number;
  readonly state: CombatState;
  readonly away: boolean;
}

export interface Stack {
  readonly id: number;
  readonly name: string;
  readonly qty: number;
}

export interface TempItem extends Stack {
  readonly maxStack: number;
}

export interface Item extends TempItem {
  readonly charItemId: number;
  readonly type: string;
  readonly slot: string;
  readonly pool: "bag" | "misc" | "class" | "house";
  readonly meta: string;
  readonly equipped: boolean;
  readonly coins: boolean;
  readonly member: boolean;
  readonly enhancement: string | null;
}

export interface Drop extends TempItem {
  readonly type: string;
  readonly pool: Item["pool"];
  readonly coins: boolean;
  readonly member: boolean;
}

export interface Quest {
  readonly id: number;
  readonly name: string;
  readonly status: "available" | "locked" | "done" | "done_this_period";
  readonly accepted: boolean;
  /** How many times quests.complete would succeed now. */
  readonly turnIns: number;
  /** Held, not taken. */
  readonly required: readonly Stack[];
  /** Taken on turn-in. */
  readonly turnIn: readonly Stack[];
  readonly requiredClass: { readonly id: number; readonly points: number } | null;
  readonly gold: number;
  readonly exp: number;
  readonly rewards: readonly (Stack & { readonly kind: "always" | "rolled" | "chosen" | "random" })[];
}

export interface ShopItem {
  readonly listingId: number;
  readonly id: number;
  readonly name: string;
  /** Units one purchase gives. */
  readonly qty: number;
  readonly cost: number;
  readonly coins: boolean;
  readonly member: boolean;
  readonly merge: readonly Stack[];
  readonly token: { readonly id: number; readonly qty: number } | null;
}

export interface Shop {
  readonly id: number;
  readonly name: string;
  readonly items: readonly ShopItem[];
}

export interface Server {
  readonly name: string;
  readonly online: boolean;
  readonly members: boolean;
  readonly players: number;
  readonly capacity: number;
}

export interface QuestRegistration {
  readonly id: number;
  readonly reward?: ItemRef;
}

export type AuraHolder = { readonly kind: "player" } | { readonly kind: "monster"; readonly monMapId: number };

export interface Events {
  joined: { readonly map: string; readonly cell: string; readonly pad: string };
  moved: { readonly cell: string; readonly pad: string };
  playerEntered: { readonly name: string };
  playerLeft: { readonly name: string };
  playerMoved:
    | { readonly kind: "jump"; readonly name: string; readonly cell: string; readonly pad: string }
    | { readonly kind: "walk"; readonly name: string; readonly cell: string; readonly x: number; readonly y: number };
  zone: { readonly zone: string | null };
  chat: { readonly channel: string; readonly sender: string; readonly text: string };
  whisper: { readonly sender: string; readonly text: string };
  notice: { readonly kind: "server" | "notify" | "warning"; readonly text: string };
  killed: { readonly monMapId: number; readonly name: string; readonly gold: number; readonly exp: number };
  died: {};
  respawned: {};
  leveledUp: { readonly level: number };
  dropOffered: Drop;
  /** `qty` is the new total in that place. */
  itemChanged: Stack & { readonly place: "inventory" | "temp" | "bank" };
  questAccepted: { readonly id: number };
  questCompleted: { readonly id: number };
  /** Auras that run out raise no event. */
  auraAdded: { readonly on: AuraHolder; readonly name: string; readonly stacks: number };
  auraRemoved: { readonly on: AuraHolder; readonly name: string };
  counterStarted: { readonly monMapId: number };
  counterEnded: { readonly monMapId: number };
  awayChanged: { readonly away: boolean };
}

export type Handler<T> = (event: T) => void | Action<void>;

/** `b.o` of each JSON command aqx parses. Other commands get `unknown`. */
export interface JsonPackets {}

/** The fields after the room of each XT command aqx parses. Other commands get strings. */
export interface XtPackets {}

export type PacketMatch =
  | { readonly kind: "json"; readonly cmd: string }
  | { readonly kind: "xt"; readonly cmd: string };

export type PacketOf<M extends PacketMatch> = M extends { readonly kind: "json"; readonly cmd: infer C }
  ? { readonly cmd: C; readonly data: C extends keyof JsonPackets ? JsonPackets[C] : unknown }
  : M extends { readonly cmd: infer C }
    ? { readonly cmd: C; readonly args: C extends keyof XtPackets ? XtPackets[C] : readonly string[] }
    : never;

export type Scalar = string | number | boolean;
export type ArgValue = Scalar | readonly Scalar[] | Readonly<Record<string, Scalar>>;

type Defaulted<T> = { [K in keyof T]-?: {} extends Pick<T, K> ? K : never }[keyof T];
type Defaults<T> = { readonly [K in Defaulted<T>]-?: Exclude<T[K], undefined> };
type Args<T> = { readonly [K in keyof T]-?: Exclude<T[K], undefined> };

interface PlayerFields {
  readonly entityId: number;
  readonly charId: number;
  readonly name: string;
  readonly level: number;
  readonly exp: number;
  readonly maxExp: number;
  readonly hp: number;
  readonly maxHp: number;
  readonly mp: number;
  readonly maxMp: number;
  readonly state: CombatState;
  readonly away: boolean;
  readonly gold: number;
  readonly coins: number;
  readonly member: boolean;
  readonly class: string;
  readonly classRank: number;
  readonly stats: Stats;
  readonly auras: readonly Aura[];
  readonly factions: readonly Faction[];
  /** Milliseconds left. */
  readonly boosts: { readonly xp: number; readonly gold: number; readonly rep: number; readonly classPoints: number };
}

interface MapFields {
  readonly cells: readonly string[];
  /** With the room, as in "battleon-1234". */
  readonly name: string;
  readonly areaId: number;
  readonly cell: string;
  readonly pad: string;
  readonly x: number | null;
  readonly y: number | null;
  /** The floor zone a boss marked. */
  readonly zone: string | null;
  readonly players: readonly Player[];
  readonly monsters: readonly Monster[];
}

interface CombatFields {
  readonly skills: readonly Skill[];
  readonly target: Monster | null;
}

interface InventoryFields {
  readonly items: readonly Item[];
  readonly temp: readonly TempItem[];
  readonly bagSlots: number;
  readonly houseSlots: number;
}

interface BankFields {
  readonly items: readonly Item[] | null;
  readonly slots: number;
}

interface DropsFields {
  readonly pending: readonly Drop[];
}

interface ShopFields {
  readonly current: Shop | null;
}

interface Settings {
  /**
   * The room map.join uses when the name has none. null, the default, picks
   * a random room from 1000 to 99999 on each join into another map.
   */
  room: number | null;
  antiAfk: boolean;
  /**
   * While a monster in the player's cell counters, `all` holds every cast,
   * auto attacks included, and `attacks` holds only the casts that would hit
   * a countering monster. A held cast goes out once the counter ends.
   */
  counterHold: "off" | "all" | "attacks";
  aggro: "off" | "cell" | readonly MonsterRef[];
  /**
   * After a dropped connection, log back in and start the script again in a
   * fresh context. Starts as the account's auto_relogin. Off, the account stays
   * logged out.
   */
  autoRelogin: boolean;
}

type Task = Generator<Step, unknown, unknown>;
type Spawned = [tag: number, task: Task];
type Listener = (event: never) => void | Action<void>;
type Op = { readonly kind: string; readonly [field: string]: unknown };

interface Native {
  readonly aqxVersion: string;
  versionCompare(left: string, right: string): -1 | 0 | 1;
  init(
    step: typeof STEP,
    error: typeof AqxError,
    dispatchEvent: (name: string, payload: unknown) => Spawned[],
    dispatchPacket: (kind: string, cmd: string, packet: unknown) => Spawned[],
    edit: (json: string) => void,
  ): void;
  log(text: string): void;
  warn(text: string): void;
  error(text: string): void;
  status(text: string): void;
  registered(json: string): void;
  stop(reason: string | undefined): void;
  restart(): void;
  version(): number;
  view(name: "player"): PlayerFields | null;
  view(name: "map"): MapFields | null;
  view(name: "combat"): CombatFields | null;
  view(name: "inventory"): InventoryFields | null;
  view(name: "bank"): BankFields | null;
  view(name: "drops"): DropsFields | null;
  view(name: "shop"): ShopFields | null;
  quest(id: number): Quest | null;
  hasRoom(id: number, pool: string): boolean;
  setCounterHold(hold: Settings["counterHold"]): boolean;
  setAggro(mode: "off" | "cell" | "only", ids: number[]): boolean;
  setAntiAfk(on: boolean): boolean;
  setRoom(room: number | null): boolean;
  setAutoRelogin(on: boolean): boolean;
  autoRelogin(): boolean;
  setSpawnPoint(cell: string, pad: string): boolean;
  untarget(): boolean;
  server(): string;
  args(): Readonly<Record<string, ArgValue>> | undefined;
  spawn(tag: number, task: Task): void;
  halt(tag: number): void;
  watchPackets(count: number): void;
}

import * as raw from "aqx:native";

const native: Native = raw;

export const AQX_VERSION: string = native.aqxVersion;

/**
 * Compares semantic versions, ignoring build metadata.
 *
 * @returns -1, 0, or 1 when left is older, equal, or newer than right.
 * @throws {RangeError} If either version is invalid or a major, minor, or patch
 * number exceeds 18446744073709551615.
 * @throws {TypeError} If either argument is not a string.
 */
export function versionCompare(left: string, right: string): -1 | 0 | 1 {
  return native.versionCompare(checkText(left, "left version"), checkText(right, "right version"));
}

let aqxError: (message: string) => AqxError;

export class AqxError extends Error {
  private constructor(message: string) {
    super(message);
    this.name = "AqxError";
  }

  static {
    aqxError = (message) => new AqxError(message);
  }
}

const eventHandlers = new Map<string, { tag: number; handler: Listener }[]>();
const packetHandlers = new Map<string, { tag: number; handler: Listener }[]>();
let nextTag = 1;

function started(entries: readonly { tag: number; handler: Listener }[], payload: unknown): Spawned[] {
  const actions: Spawned[] = [];
  for (const { tag, handler } of [...entries]) {
    const result = handler(payload as never);
    if (isAction(result)) actions.push([tag, delegate(result)]);
  }
  return actions;
}

type Edit =
  | { readonly register_drop: ItemRef }
  | { readonly unregister_drop: ItemRef }
  | { readonly register_quest: QuestRegistration }
  | { readonly unregister_quest: number };

function edit(json: string): void {
  const change = JSON.parse(json) as Edit;
  if ("register_drop" in change) drops.register(change.register_drop);
  else if ("unregister_drop" in change) drops.unregister(change.unregister_drop);
  else if ("register_quest" in change) quests.register(change.register_quest);
  else quests.unregister(change.unregister_quest);
}

native.init(
  STEP,
  AqxError,
  (name, payload) => started(eventHandlers.get(name) ?? [], payload),
  (kind, cmd, packet) => started(packetHandlers.get(`${kind}:${cmd}`) ?? [], packet),
  edit,
);

function listen(table: Map<string, { tag: number; handler: Listener }[]>, key: string, handler: Listener): number {
  const tag = nextTag++;
  const entries = table.get(key) ?? [];
  entries.push({ tag, handler });
  table.set(key, entries);
  return tag;
}

function unlisten(table: Map<string, { tag: number; handler: Listener }[]>, key: string, handler: Listener): void {
  const entries = table.get(key) ?? [];
  const at = entries.findIndex((entry) => entry.handler === handler);
  if (at < 0) return;
  const [{ tag }] = entries.splice(at, 1);
  if (entries.length === 0) table.delete(key);
  native.halt(tag);
}

function spawn(task: Action<unknown>): number {
  const tag = nextTag++;
  native.spawn(tag, delegate(task));
  return tag;
}

const unstarted = new Map<Action<unknown>, string | undefined>();

function runnable<T>(run: () => Generator<Step, T, unknown>): Action<T> {
  const value: Action<T> = {
    *[Symbol.iterator]() {
      unstarted.delete(value);
      return yield* run();
    },
  };
  if (unstarted.size === 0) Promise.resolve().then(reportUnstarted);
  unstarted.set(value, new Error().stack);
  return value;
}

function reportUnstarted(): void {
  for (const stack of unstarted.values()) {
    const caller = (stack ?? "")
      .split("\n")
      .map((line) => /\((.+)\)$/.exec(line.trim())?.[1])
      .find((at) => at !== undefined && !at.startsWith("aqx") && at !== "native");
    native.warn(`an action${caller === undefined ? "" : ` at ${caller}`} never ran; run it with yield*`);
  }
  unstarted.clear();
}

function action<T>(op: () => Op): Action<T> {
  return runnable(function* (): Generator<Step, T, unknown> {
    const result: unknown = yield { [STEP]: op() };
    return result as T;
  });
}

function act<T = void>(name: string, args: Readonly<Record<string, unknown>> = {}): Action<T> {
  return action(() => ({ kind: "act", name, ...args }));
}

function isAction(value: unknown): value is Action<unknown> {
  return (
    (typeof value === "object" || typeof value === "function") &&
    value !== null &&
    Symbol.iterator in value &&
    typeof value[Symbol.iterator] === "function"
  );
}

function delegate(action: Action<unknown>): Task {
  unstarted.delete(action);
  return (function* () {
    return yield* action;
  })();
}

function checkText(value: unknown, what: string): string {
  if (typeof value !== "string") throw new TypeError(`${what} must be a string`);
  return value;
}

function checkId(value: unknown, what: string): number {
  if (typeof value !== "number") throw new TypeError(`${what} must be a number`);
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function checkCount(value: unknown, what: string): number {
  if (typeof value !== "number") throw new TypeError(`${what} must be a number`);
  if (!Number.isInteger(value) || value < 1) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function checkCoordinate(value: unknown, axis: string): number {
  if (typeof value !== "number") throw new TypeError(`map.walk's ${axis} must be a number`);
  if (!Number.isInteger(value)) throw new RangeError(`map.walk can't walk to ${axis} ${value}`);
  return value;
}

function checkRef(value: unknown, what: string, kind: string): number | string {
  if (typeof value === "string") return value;
  if (typeof value !== "number") throw new TypeError(`${what} must be ${kind}`);
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} can't be ${value}`);
  return value;
}

function monsterRef(value: unknown, what: string): MonsterRef {
  return checkRef(value, what, "a monMapId or a monster name");
}

function playerRef(value: unknown, what: string): PlayerRef {
  return checkRef(value, what, "an entityId or a player name");
}

function itemRef(value: unknown, what: string): ItemRef {
  return checkRef(value, what, "an item id or an item name");
}

function reader<T>(read: () => T | null, missing: string): () => T {
  let seen = -1;
  let value: T | null = null;
  return () => {
    const version = native.version();
    if (version !== seen) {
      value = read();
      seen = version;
    }
    if (value === null) throw aqxError(missing);
    return value;
  };
}

function view<T extends object, M extends object>(
  read: () => T,
  fields: { readonly [K in keyof T]-?: true },
  methods: M,
): T & M {
  for (const name in fields) {
    Object.defineProperty(methods, name, { get: () => read()[name], enumerable: true });
  }
  return Object.freeze(methods) as T & M;
}

const notLoaded = "the character has not loaded yet";
const me = reader(() => native.view("player"), notLoaded);
const room = reader(() => native.view("map"), "the player is not in a map yet");
const fighting = reader(() => native.view("combat"), notLoaded);
const bag = reader(() => native.view("inventory"), notLoaded);
const vault = reader(() => native.view("bank"), notLoaded);
const offers = reader(() => native.view("drops"), notLoaded);
const store = reader(() => native.view("shop"), notLoaded);

function session(done: boolean): void {
  if (!done) throw aqxError("the session isn't running yet");
}

export const player = view(
  me,
  {
    entityId: true, charId: true, name: true, level: true, exp: true, maxExp: true, hp: true,
    maxHp: true, mp: true, maxMp: true, state: true, away: true, gold: true, coins: true,
    member: true, class: true, classRank: true, stats: true, auras: true, factions: true,
    boosts: true,
  },
  {
    rest(): Action<void> {
      return act("rest");
    },
  },
);

export const map = view(
  room,
  { cells: true, name: true, areaId: true, cell: true, pad: true, x: true, y: true, zone: true, players: true, monsters: true },
  {
    /**
     * Without a room in `name`, joins settings.room. Leaves combat first, and
     * waits for the respawn when the player is dead.
     * "refused" when the map is locked, full, or for members only.
     */
    join(name: string, options: { readonly cell?: string; readonly pad?: string } = {}): Action<"joined" | "refused"> {
      const map = checkText(name, "map.join's map name");
      const cell = options.cell === undefined ? undefined : checkText(options.cell, "map.join's cell");
      const pad = options.pad === undefined ? undefined : checkText(options.pad, "map.join's pad");
      if (pad !== undefined && cell === undefined) throw new TypeError("map.join's pad needs a cell");
      return act("join", { map, cell, pad });
    },
    jump(cell: string, pad?: string): Action<void> {
      return act("jump", { cell: checkText(cell, "map.jump's cell"), pad: pad === undefined ? undefined : checkText(pad, "map.jump's pad") });
    },
    /** Skipped if the player changes map or cell before it's sent. */
    walk(x: number, y: number): Action<void> {
      return act("walk", { x: checkCoordinate(x, "x"), y: checkCoordinate(y, "y") });
    },
    /**
     * Leaves combat first when the player is in another map, and waits for the
     * respawn when the player is dead. "not_found" when the player isn't on
     * this server. Another account in this aqx session on
     * the same server is reached by joining its room, which also works in maps
     * where the server refuses goto.
     */
    goto(player: string): Action<"arrived" | "not_found" | "refused"> {
      return act("goto", { player: checkText(player, "map.goto's player") });
    },
    /** Finishes once sent. */
    collect(mapItemId: number): Action<void> {
      return act("collect", { item: checkId(mapItemId, "map.collect's mapItemId") });
    },
    /**
     * Where the player respawns in this map when the server doesn't name a
     * cell. Each join resets it to Enter, Spawn. `pad` defaults to "Left".
     */
    setSpawnPoint(cell: string, pad: string = "Left"): void {
      session(native.setSpawnPoint(checkText(cell, "map.setSpawnPoint's cell"), checkText(pad, "map.setSpawnPoint's pad")));
    },
  },
);

function alive(monster: Monster): boolean {
  return monster.state !== "dead" && monster.hp > 0;
}

function matching(target: MonsterRef): Monster[] {
  const { cell, monsters } = room();
  return monsters.filter(
    (monster) =>
      monster.cell === cell &&
      (typeof target === "number" ? monster.monMapId === target : monster.name.toLowerCase() === target.toLowerCase()),
  );
}

function slots(value: unknown): number[] {
  if (!Array.isArray(value)) throw new TypeError("combat.kill's skills must be an array of slots");
  if (value.length === 0) throw new RangeError("combat.kill needs at least one skill");
  const checked: number[] = [];
  for (const slot of value) {
    if (typeof slot !== "number") throw new TypeError("combat.kill's skills must be slot numbers");
    if (!Number.isInteger(slot) || slot < 1 || slot > 5) throw new RangeError(`combat.kill can't use slot ${slot}`);
    checked.push(slot);
  }
  return checked;
}

export const combat = view(
  fighting,
  { skills: true, target: true },
  {
    /**
     * Fights the live match with the least HP in the player's cell, casting
     * `skills` in turn (default 1-4). "none" when no match is alive. With
     * `waitForRespawn`, it waits for a dead match in the cell to come back,
     * and returns "none" only when the cell has no match at all.
     */
    kill(
      target: MonsterRef,
      options: { readonly skills?: readonly Exclude<Slot, 0>[]; readonly waitForRespawn?: boolean } = {},
    ): Action<"killed" | "died" | "none"> {
      const monster = monsterRef(target, "combat.kill's target");
      const skills = options.skills === undefined ? undefined : slots(options.skills);
      const kill = () => act<"killed" | "died" | "none">("kill", { target: monster, skills });
      if (!options.waitForRespawn) return kill();
      return runnable(function* () {
        const settled = () => matching(monster).length === 0 || matching(monster).some(alive);
        if (!settled()) yield* wait.until(settled);
        if (matching(monster).length === 0) return "none";
        return yield* kill();
      });
    },
    attack(target: MonsterRef): Action<void> {
      return act("attack", { target: monsterRef(target, "combat.attack's target") });
    },
    /**
     * A friendly skill aims at `player`, who must be alive in the player's
     * cell, or else at the player. A hostile skill can't aim at a player.
     */
    useSkill(
      slot: Exclude<Slot, 0>,
      target?:
        | { readonly monster: MonsterRef; readonly player?: never }
        | { readonly player: PlayerRef; readonly monster?: never },
    ): Action<void> {
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
    exit(): Action<void> {
      return act("exit");
    },
    untarget(): void {
      session(native.untarget());
    },
  },
);

function find<T extends Stack>(items: readonly T[], item: ItemRef): T | undefined {
  if (typeof item === "number") return items.find((each) => each.id === item);
  const name = item.toLowerCase();
  return items.find((each) => each.name.toLowerCase() === name);
}

export const inventory = view(
  bag,
  { items: true, temp: true, bagSlots: true, houseSlots: true },
  {
    get(item: ItemRef): Item | undefined {
      return find(bag().items, itemRef(item, "inventory.get's item"));
    },
    /** Items and temp items together, as quests count them. */
    count(item: ItemRef): number {
      const wanted = itemRef(item, "inventory.count's item");
      return (find(bag().items, wanted) ?? find(bag().temp, wanted))?.qty ?? 0;
    },
    canFit(drop: Drop): boolean {
      if (drop === null || typeof drop !== "object") throw new TypeError("inventory.canFit takes a drop");
      return native.hasRoom(checkId(drop.id, "inventory.canFit's drop id"), checkText(drop.pool, "inventory.canFit's drop pool"));
    },
    equip(item: ItemRef): Action<void> {
      return act("equip", { item: itemRef(item, "inventory.equip's item") });
    },
    unequip(item: ItemRef): Action<void> {
      return act("unequip", { item: itemRef(item, "inventory.unequip's item") });
    },
    /** For the slotted consumable, use combat.useSkill(5). */
    use(item: ItemRef): Action<void> {
      return act("use", { item: itemRef(item, "inventory.use's item") });
    },
  },
);

export const bank = view(
  vault,
  { items: true, slots: true },
  {
    load(): Action<readonly Item[]> {
      return act("loadBank");
    },
    deposit(item: ItemRef): Action<void> {
      return act("deposit", { item: itemRef(item, "bank.deposit's item") });
    },
    withdraw(item: ItemRef): Action<void> {
      return act("withdraw", { item: itemRef(item, "bank.withdraw's item") });
    },
    swap(inventoryItem: ItemRef, bankItem: ItemRef): Action<void> {
      return act("swap", {
        item: itemRef(inventoryItem, "bank.swap's inventory item"),
        bankItem: itemRef(bankItem, "bank.swap's bank item"),
      });
    },
  },
);

const wantedDrops: ItemRef[] = [];
let dropsWatched = false;

function reportRegistered(): void {
  native.registered(
    JSON.stringify({ drops: wantedDrops, quests: [...questLoops.values()].map((loop) => loop.registration) }),
  );
}

function wanted(offer: Drop): boolean {
  return wantedDrops.some((item) =>
    typeof item === "number" ? offer.id === item : offer.name.toLowerCase() === item.toLowerCase(),
  );
}

function* pickupLogged(offer: Drop): Script {
  try {
    if ((yield* drops.pickup(offer.id)) === "no_room") script.warn(`no room to pick up ${offer.name}`);
  } catch (e) {
    if (!(e instanceof AqxError)) throw e;
    script.warn(`couldn't pick up ${offer.name}: ${e.message}`);
  }
}

export const drops = view(
  offers,
  { pending: true },
  {
    /** "gone" when the offer is no longer pending. */
    pickup(item: ItemRef): Action<"picked" | "no_room" | "gone"> {
      return act("pickup", { item: itemRef(item, "drops.pickup's item") });
    },
    get registered(): readonly ItemRef[] {
      return [...wantedDrops];
    },
    /** Includes offers already pending. A failed pickup is logged and stays pending. */
    register(...items: ItemRef[]): void {
      const refs = items.map((item) => itemRef(item, "drops.register's item"));
      for (const item of refs) if (!wantedDrops.includes(item)) wantedDrops.push(item);
      reportRegistered();
      if (!dropsWatched) {
        dropsWatched = true;
        listen(eventHandlers, "dropOffered", (offer: Drop) => (wanted(offer) ? pickupLogged(offer) : undefined));
      }
      for (const offer of offers().pending) if (wanted(offer)) spawn(pickupLogged(offer));
    },
    unregister(...items: ItemRef[]): void {
      for (const item of items) {
        const at = wantedDrops.indexOf(itemRef(item, "drops.unregister's item"));
        if (at >= 0) wantedDrops.splice(at, 1);
      }
      reportRegistered();
    },
  },
);

const questLoops = new Map<number, { registration: QuestRegistration; tag: number }>();

function* questLoop(registration: QuestRegistration): Script {
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
    reportRegistered();
  }
}

function registration(value: number | QuestRegistration): QuestRegistration {
  if (typeof value === "number") return { id: checkId(value, "quests.register's id") };
  if (value === null || typeof value !== "object") throw new TypeError("quests.register takes quest ids or { id, reward }");
  const questId = checkId(value.id, "quests.register's id");
  if (value.reward === undefined) return Object.freeze({ id: questId });
  return Object.freeze({ id: questId, reward: itemRef(value.reward, "quests.register's reward") });
}

export const quests = Object.freeze({
  get(id: number): Quest | undefined {
    return native.quest(checkId(id, "quests.get's id")) ?? undefined;
  },
  load(...ids: number[]): Action<void> {
    if (ids.length === 0) throw new TypeError("quests.load needs at least one quest id");
    return act("loadQuests", { ids: ids.map((questId) => checkId(questId, "quests.load's id")) });
  },
  accept(id: number): Action<void> {
    return act("accept", { id: checkId(id, "quests.accept's id") });
  },
  complete(id: number, options: { readonly reward?: ItemRef; readonly times?: number } = {}): Action<void> {
    const reward = options.reward === undefined ? undefined : itemRef(options.reward, "quests.complete's reward");
    const times = options.times === undefined ? 1 : checkCount(options.times, "quests.complete's times");
    return act("complete", { id: checkId(id, "quests.complete's id"), reward, times });
  },
  get registered(): readonly QuestRegistration[] {
    return [...questLoops.values()].map((loop) => loop.registration);
  },
  /**
   * Accepts each quest, turns it in whenever it can, and accepts it again.
   * A quest that fails is logged and unregistered.
   */
  register(...quests: (number | QuestRegistration)[]): void {
    for (const registered of quests.map(registration)) {
      const running = questLoops.get(registered.id);
      if (running) native.halt(running.tag);
      questLoops.set(registered.id, { registration: registered, tag: spawn(questLoop(registered)) });
    }
    reportRegistered();
  },
  unregister(...ids: number[]): void {
    for (const questId of ids.map((value) => checkId(value, "quests.unregister's id"))) {
      const running = questLoops.get(questId);
      if (!running) continue;
      questLoops.delete(questId);
      native.halt(running.tag);
    }
    reportRegistered();
  },
});

export const shop = view(
  store,
  { current: true },
  {
    load(id: number): Action<Shop> {
      return act("loadShop", { id: checkId(id, "shop.load's id") });
    },
    /** `qty` counts units. An item two listings sell needs the listing. */
    buy(item: ItemRef | { readonly listingId: number }, qty?: number): Action<void> {
      const units = qty === undefined ? undefined : checkCount(qty, "shop.buy's qty");
      if (item !== null && typeof item === "object") {
        return act("buy", { listingId: checkId(item.listingId, "shop.buy's listingId"), qty: units });
      }
      return act("buy", { item: itemRef(item, "shop.buy's item"), qty: units });
    },
    sell(item: ItemRef, qty?: number): Action<void> {
      const units = qty === undefined ? 1 : checkCount(qty, "shop.sell's qty");
      return act("sell", { item: itemRef(item, "shop.sell's item"), qty: units });
    },
  },
);

export const wait = {
  ms(ms: number): Action<void> {
    if (typeof ms !== "number") throw new TypeError("wait.ms takes a number of milliseconds");
    if (!(ms >= 0 && ms < Infinity)) throw new RangeError(`wait.ms can't wait ${ms} ms`);
    return action(() => ({ kind: "ms", ms }));
  },
  /** Checks now and whenever state changes. */
  until(predicate: () => boolean): Action<void> {
    if (typeof predicate !== "function") throw new TypeError("wait.until takes a function");
    return action(() => ({ kind: "until", predicate }));
  },
  /** Runs these together until one finishes, then stops the rest. */
  first<R extends Record<string, Action<unknown>>>(
    actions: R,
  ): Action<{ [K in keyof R]: { readonly winner: K; readonly value: ValueOf<R[K]> } }[keyof R]> {
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

const EVENTS: { readonly [K in keyof Events]-?: true } = {
  joined: true, moved: true, playerEntered: true, playerLeft: true, playerMoved: true, zone: true,
  chat: true, whisper: true, notice: true, killed: true, died: true, respawned: true,
  leveledUp: true, dropOffered: true, itemChanged: true, questAccepted: true,
  questCompleted: true, auraAdded: true, auraRemoved: true, counterStarted: true,
  counterEnded: true, awayChanged: true,
};

function isEvent(name: unknown): name is keyof Events {
  return typeof name === "string" && Object.hasOwn(EVENTS, name);
}

function eventName(name: unknown, what: string): keyof Events {
  if (!isEvent(name)) throw new TypeError(`${what}: ${String(name)} isn't an event`);
  return name;
}

function handlerOf<F>(handler: F, what: string): F {
  if (typeof handler !== "function") throw new TypeError(`${what} takes a function`);
  return handler;
}

function filterOf<F>(filter: F | undefined, what: string): F | undefined {
  if (filter !== undefined && typeof filter !== "function") throw new TypeError(`${what}'s filter must be a function`);
  return filter;
}

export const events = Object.freeze({
  on<K extends keyof Events>(event: K, handler: Handler<Events[K]>): void {
    listen(eventHandlers, eventName(event, "events.on"), handlerOf(handler, "events.on"));
  },
  off<K extends keyof Events>(event: K, handler: Handler<Events[K]>): void {
    unlisten(eventHandlers, eventName(event, "events.off"), handler);
  },
  once<K extends keyof Events>(event: K, filter?: (event: Events[K]) => boolean): Action<Events[K]> {
    const name = eventName(event, "events.once");
    return action(() => ({ kind: "once", event: name, filter: filterOf(filter, "events.once") }));
  },
});

function packetKey(match: unknown, what: string): { kind: "json" | "xt"; cmd: string } {
  if (match === null || typeof match !== "object") throw new TypeError(`${what} takes { kind, cmd }`);
  const kind = "kind" in match ? match.kind : undefined;
  if (kind !== "json" && kind !== "xt") throw new TypeError(`${what}'s kind must be "json" or "xt"`);
  return { kind, cmd: checkText("cmd" in match ? match.cmd : undefined, `${what}'s cmd`) };
}

function watchedPackets(): void {
  let watching = 0;
  for (const entries of packetHandlers.values()) watching += entries.length;
  native.watchPackets(watching);
}

export const packets = Object.freeze({
  /** Runs after aqx has applied the message to state. */
  on<const M extends PacketMatch>(match: M, handler: Handler<PacketOf<M>>): void {
    const { kind, cmd } = packetKey(match, "packets.on");
    listen(packetHandlers, `${kind}:${cmd}`, handlerOf(handler, "packets.on"));
    watchedPackets();
  },
  off<const M extends PacketMatch>(match: M, handler: Handler<PacketOf<M>>): void {
    const { kind, cmd } = packetKey(match, "packets.off");
    unlisten(packetHandlers, `${kind}:${cmd}`, handler);
    watchedPackets();
  },
  /** Started right after `send`, it sees the reply. */
  once<const M extends PacketMatch>(match: M, filter?: (packet: PacketOf<M>) => boolean): Action<PacketOf<M>> {
    const { kind, cmd } = packetKey(match, "packets.once");
    return action(() => ({ kind: "packet", packetKind: kind, cmd, filter: filterOf(filter, "packets.once") }));
  },
  /** Queued behind earlier actions. aqx can't pace a raw packet the way the client would. */
  send(text: string): Action<void> {
    return act("send", { text: checkText(text, "packets.send's packet") });
  },
});

const COUNTER_HOLDS: readonly Settings["counterHold"][] = ["off", "all", "attacks"];
const config: Omit<Settings, "autoRelogin"> & { autoRelogin?: boolean } = { room: null, antiAfk: false, counterHold: "off", aggro: "off" };
let aggroWatched = false;

function applyAggro(): boolean {
  const aggro = config.aggro;
  if (aggro === "off" || aggro === "cell") return native.setAggro(aggro, []);
  const ids = new Set<number>();
  for (const monster of room().monsters) {
    if (aggro.some((target) => (typeof target === "number" ? monster.monMapId === target : monster.name.toLowerCase() === target.toLowerCase()))) {
      ids.add(monster.monMapId);
    }
  }
  for (const target of aggro) if (typeof target === "number") ids.add(target);
  return native.setAggro("only", [...ids]);
}

/** Values go back to what they were before the run when it ends. */
export const settings: Settings = Object.seal({
  get room() {
    return config.room;
  },
  set room(value) {
    if (value !== null) checkId(value, "settings.room");
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
      aggro = Object.freeze(aggro.map((target) => monsterRef(target, "settings.aggro's monster")));
    }
    config.aggro = aggro;
    session(applyAggro());
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
    return config.autoRelogin ?? native.autoRelogin();
  },
  set autoRelogin(on) {
    if (typeof on !== "boolean") throw new TypeError("settings.autoRelogin must be true or false");
    native.setAutoRelogin(on);
    config.autoRelogin = on;
  },
});

export const auth = Object.freeze({
  get server(): string {
    return native.server();
  },
  servers(): Action<readonly Server[]> {
    return action(() => ({ kind: "servers" }));
  },
  relogin(server?: string): Action<void> {
    const name = server === undefined ? undefined : checkText(server, "auth.relogin's server");
    return action(() => ({ kind: "relogin", server: name }));
  },
  /** Ends the run. */
  logout(): Action<never> {
    return action(() => ({ kind: "logout" }));
  },
});

function show(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error || typeof value !== "object" || value === null) return String(value);
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function say(emit: (text: string) => void): (...args: unknown[]) => void {
  return (...args) => emit(args.map(show).join(" "));
}

export const script = {
  /**
   * Optional keys take their default from `defaults`. aqx reads `T` from the
   * source to parse and ask for values, so write it inline. Lists and maps
   * are JSON on the CLI: `--arg monsters='["Treeant"]'`.
   */
  args<T extends Record<string, ArgValue | undefined>>(
    ...defaults: [Defaulted<T>] extends [never] ? [] : [Defaults<T>]
  ): Args<T> {
    const [fallback = {}]: readonly unknown[] = defaults;
    if (fallback === null || typeof fallback !== "object") throw new TypeError("script.args takes an object of defaults");
    const given = native.args();
    if (given === undefined) throw aqxError("script.args can't run before the script starts");
    return Object.freeze({ ...fallback, ...given }) as Args<T>;
  },
  status(text: string): void {
    native.status(String(text));
  },
  log: say(native.log),
  warn: say(native.warn),
  error: say(native.error),
  stop(reason?: string): never {
    const text = reason === undefined ? undefined : String(reason);
    native.stop(text);
    throw aqxError(text ?? "the script stopped");
  },
  /** Ends the run and starts the script again in a fresh context, with the same args. */
  restart(): never {
    native.restart();
    throw aqxError("the script is restarting");
  },
};
