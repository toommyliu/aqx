/*
 * Every function in this module blocks until it is done. While a call blocks,
 * the client keeps reading the socket, fighting and running event handlers.
 * Functions passed as predicates (`until`, `when`, `hold`, `DropRule`s, `once`
 * filters) must not call anything that blocks; doing so throws.
 *
 * Everything a script sets up (handlers, registered quests and drops, aggro,
 * settings) ends with the script. The bot stays logged in for the next one.
 */
declare module "aqx" {
  /** Item name (case-insensitive, exact) or ItemID. */
  export type ItemRef = string | number;

  /**
   * Monster name (case-insensitive, `"*"` = any), MonMapID, an ordered list of
   * those (earlier entries are attacked first), or a predicate.
   */
  export type Target = string | number | readonly (string | number)[] | ((m: Monster) => boolean);

  /** 0 = auto attack, 1-4 = class skills, 5 = equipped consumable. */
  export type Slot = 0 | 1 | 2 | 3 | 4 | 5;

  export type State = "dead" | "idle" | "combat";

  export type ErrorCode = "timeout" | "rejected" | "not_found" | "inventory_full" | "cancelled";

  export interface Stats {
    readonly str: number;
    readonly int: number;
    readonly dex: number;
    readonly wis: number;
    readonly end: number;
    readonly luk: number;
    /** Fractions, e.g. 0.25 = 25%. `haste` is capped at the 0.5 the server honours. */
    readonly haste: number;
    readonly crit: number;
    readonly hit: number;
    readonly dodge: number;
  }

  export interface Aura {
    readonly name: string;
    readonly stacks: number;
    /** `Date.now()` timestamp. */
    readonly expiresAt: number;
  }

  export interface Faction {
    readonly id: number;
    readonly name: string;
    readonly rep: number;
    readonly rank: number;
  }

  export interface Monster {
    /** Instance id within the map. Numeric `Target`s refer to this. */
    readonly mapId: number;
    /** Species id, shared by every instance of the same monster. */
    readonly monId: number;
    readonly name: string;
    readonly cell: string;
    readonly hp: number;
    readonly maxHp: number;
    readonly alive: boolean;
    readonly auras: readonly Aura[];
  }

  export interface OtherPlayer {
    readonly name: string;
    readonly level: number;
    readonly cell: string;
    readonly pad: string;
    readonly hp: number;
    readonly maxHp: number;
    readonly state: State;
    readonly afk: boolean;
  }

  export interface Skill {
    readonly slot: Slot;
    readonly name: string;
    readonly mana: number;
    /** Effective cooldown in ms after haste. */
    readonly cooldown: number;
    /** 0 when castable now. */
    readonly readyIn: number;
    readonly kind: "hostile" | "friendly" | "self";
    readonly maxTargets: number;
  }

  export interface Item {
    readonly id: number;
    readonly name: string;
    readonly qty: number;
    readonly maxStack: number;
    /** Server `sType`, e.g. "Class", "Sword", "Item", "Quest Item". */
    readonly type: string;
    readonly equipped: boolean;
    readonly ac: boolean;
    readonly member: boolean;
  }

  export interface Items {
    readonly items: readonly Item[];
    get(ref: ItemRef): Item | undefined;
    count(ref: ItemRef): number;
    has(ref: ItemRef, qty?: number): boolean;
  }

  /** Drop name or ItemID, or a predicate. */
  export type DropRule = ItemRef | ((d: Drop) => boolean);

  export interface Drop {
    readonly id: number;
    readonly name: string;
    readonly qty: number;
    readonly ac: boolean;
  }

  export interface Goal {
    readonly item: ItemRef;
    /** Counted across inventory and temp inventory. */
    readonly qty: number;
  }

  export type QuestStatus = "locked" | "available" | "active" | "completable" | "done";

  export interface Requirement extends Goal {
    readonly item: number;
    readonly name: string;
  }

  export interface Quest {
    readonly id: number;
    readonly name: string;
    readonly status: QuestStatus;
    readonly requirements: readonly Requirement[];
    readonly rewards: readonly {
      readonly item: number;
      readonly name: string;
      readonly qty: number;
      /** "choice" rewards need `reward` in `quests.complete`. */
      readonly kind: "fixed" | "choice" | "roll" | "random";
    }[];
  }

  export interface ShopItem {
    readonly id: number;
    readonly name: string;
    readonly cost: number;
    readonly ac: boolean;
    readonly member: boolean;
    readonly stack: number;
  }

  export interface Shop {
    readonly id: number;
    readonly name: string;
    readonly items: readonly ShopItem[];
  }

  export interface Server {
    readonly name: string;
    readonly online: boolean;
    readonly players: number;
    readonly max: number;
    readonly member: boolean;
  }

  export type Step = Slot | { readonly slot: Slot; readonly when: () => boolean };

  export interface Fight {
    /** Default `[1, 2, 3, 4]`. When several steps are ready, the earliest wins. */
    readonly skills?: readonly Step[];
    /** Cast steps strictly in order, waiting on each cooldown. */
    readonly strict?: boolean;
    /** While this returns true, nothing is cast. */
    readonly hold?: () => boolean;
  }

  export interface KillOptions extends Fight {
    /** Fight in this cell. Without it, the bot hunts across cells for live targets. */
    readonly cell?: string;
    readonly pad?: string;
    /** Keep killing matching monsters until this returns true. */
    readonly until?: () => boolean;
    readonly timeout?: number;
  }

  export interface FarmOptions extends Fight {
    /** Default `"*"`. */
    readonly target?: Target;
    readonly cell?: string;
    readonly pad?: string;
    readonly timeout?: number;
  }

  export type Packet =
    | { readonly kind: "json"; readonly cmd: string; readonly data: unknown }
    | { readonly kind: "xt"; readonly cmd: string; readonly args: readonly string[] }
    | { readonly kind: "xml"; readonly cmd: string; readonly xml: string };

  export type AuraHolder =
    | { readonly kind: "self" }
    | { readonly kind: "monster"; readonly monster: Monster }
    | { readonly kind: "player"; readonly name: string };

  export interface EventMap {
    death: undefined;
    respawn: undefined;
    map: { readonly name: string; readonly room: number };
    cell: { readonly cell: string; readonly pad: string };
    kill: Monster;
    drop: Drop;
    item: { readonly item: Item; readonly delta: number; readonly where: "inventory" | "temp" | "bank" };
    quest: { readonly id: number; readonly change: "accepted" | "completed" };
    aura: { readonly on: AuraHolder; readonly aura: Aura; readonly change: "added" | "removed" };
    counter: { readonly active: boolean };
    chat: {
      readonly from: string;
      readonly text: string;
      readonly channel: "zone" | "party" | "guild" | "whisper";
    };
    warning: { readonly text: string };
  }

  export const player: {
    readonly name: string;
    readonly level: number;
    readonly hp: number;
    readonly maxHp: number;
    readonly mp: number;
    readonly maxMp: number;
    readonly state: State;
    readonly afk: boolean;
    readonly cell: string;
    readonly pad: string;
    readonly x: number;
    readonly y: number;
    readonly gold: number;
    readonly member: boolean;
    readonly class: { readonly name: string; readonly rank: number } | null;
    readonly target: Monster | null;
    readonly stats: Stats;
    readonly auras: readonly Aura[];
    readonly factions: readonly Faction[];
    /** Returns when HP and MP are full. Throws `rejected` if combat starts. */
    rest(): void;
  };

  export const map: {
    readonly name: string;
    readonly room: number;
    readonly monsters: readonly Monster[];
    readonly players: readonly OtherPlayer[];
    find(target: Target): Monster[];
    /**
     * `name` may carry a room suffix, as in `join("timespace-12345")`. No-op if
     * already there. Returns once the map's monsters are known and the player
     * stands in `cell`. Throws `rejected` with the server's text for locked or
     * full maps.
     */
    join(name: string, cell?: string, pad?: string): void;
    jump(cell: string, pad?: string): void;
    walk(x: number, y: number): void;
    goto(player: string): void;
    /** Picks up a map item until the temp inventory gains `qty` of it. */
    collect(mapItemId: number, qty?: number): void;
  };

  export const combat: {
    /** Returns after one matching monster dies, or when `opts.until` returns true. */
    kill(target: Target, opts?: KillOptions): void;
    /**
     * Withdraws the items from the bank, picks up their drops, shows a progress
     * bar per item, and kills until every count is reached.
     */
    farm(item: ItemRef, qty: number, opts?: FarmOptions): void;
    farm(goals: readonly Goal[], opts?: FarmOptions): void;
    /** Keeps matching monsters in the current map aggroed. Returns a function that stops it. */
    aggro(target: Target): () => void;
    exit(): void;
  };

  export const skills: {
    readonly slots: readonly Skill[];
    /** Waits until the slot is castable, then casts it. */
    use(slot: Slot, target?: Target): void;
  };

  export const inventory: Items & {
    readonly temp: Items;
    readonly slots: number;
    readonly used: number;
    /** Withdraws from the bank first if needed. Leaves combat first. */
    equip(item: ItemRef): void;
    /** Uses a consumable such as a boost. */
    use(item: ItemRef): void;
  };

  export const bank: Items & {
    readonly slots: number;
    readonly used: number;
    /** Skips items already in the inventory. */
    withdraw(...items: ItemRef[]): void;
    /** Skips items already in the bank. */
    deposit(...items: ItemRef[]): void;
  };

  export const quests: {
    /** Loads the quest on first call. Later calls return its current status without blocking. */
    get(id: number): Quest;
    /** Skips quests already active. */
    accept(...ids: number[]): void;
    /** `times` turns in that many completions at once when the quest allows it. */
    complete(id: number, opts?: { readonly reward?: number; readonly times?: number }): void;
    readonly registered: readonly number[];
    /** Accepts, completes and re-accepts in the background. Registering a quest again replaces its reward. */
    register(...quests: (number | { readonly id: number; readonly reward: number })[]): void;
    unregister(...ids: number[]): void;
  };

  export const shop: {
    /** Loads the shop on first call, then returns the cached copy. */
    get(id: number): Shop;
    buy(shop: number, item: ItemRef, qty?: number): void;
    sell(item: ItemRef, qty?: number): void;
  };

  export const drops: {
    /** Offered but not yet picked up. */
    readonly pending: readonly Drop[];
    pickup(...items: ItemRef[]): void;
    readonly registered: readonly DropRule[];
    /** Picks up matching drops, pending and future. */
    register(...rules: DropRule[]): void;
    /** Names match case-insensitively, predicates by identity. */
    unregister(...rules: DropRule[]): void;
  };

  export const events: {
    /** Handlers run while the script is inside a blocking call. Events that arrive during a handler queue until it returns. */
    on<K extends keyof EventMap>(event: K, handler: (e: EventMap[K]) => void): () => void;
    /** Throws `timeout` if `opts.timeout` elapses first. */
    once<K extends keyof EventMap>(
      event: K,
      opts?: { readonly timeout?: number; readonly filter?: (e: EventMap[K]) => boolean },
    ): EventMap[K];
  };

  export const packet: {
    /** Raw `%xt%zm%...%` packet. Goes through the same rate limiter as built-in actions. */
    send(raw: string): void;
    /** Sends, then returns the next packet whose cmd is `reply`. */
    request(raw: string, reply: string, timeout?: number): Packet;
    on(cmd: string, handler: (p: Packet) => void): () => void;
  };

  export const wait: {
    /** Re-evaluates `predicate` after every state change and at least every 250 ms. Returns `false` on timeout. */
    until(predicate: () => boolean, timeout?: number): boolean;
    ms(ms: number): void;
  };

  export const script: {
    /**
     * Script inputs. Each key's default fixes its type; `--arg key=value` on
     * the CLI or `args` in aqx.toml overrides it. Throws on unknown keys or
     * values that don't parse as the default's type.
     */
    args<T extends Record<string, string | number | boolean>>(defaults: T): T;
    /** One line describing what the script is doing. Shown in the TUI and logs. */
    status(text: string): void;
    /** Named progress bar with rate and ETA. `current` is absolute, not a delta. */
    progress(label: string, current: number, total?: number): void;
    /** Ends the script. The bot stays logged in. Catching the error it throws does not keep the script alive. */
    stop(reason?: string): never;
  };

  export const session: {
    readonly server: string;
    servers(): Server[];
    /** Reconnects, optionally to another server, and returns once the character is loaded. */
    relogin(server?: string): void;
  };

  /** Reverts to the bot's defaults when the script ends. */
  export const settings: {
    /** Room used by `map.join` when the name has no `-room` suffix. */
    room: number | null;
    /** Where to go after respawning. `null` returns to the cell the player died in. */
    respawn: { readonly cell: string; readonly pad: string } | null;
    /** Stop casting while a monster telegraphs a counter attack. Default `true`. */
    antiCounter: boolean;
  };

  export class AqxError extends Error {
    readonly code: ErrorCode;
    /** The server's text, set when `code` is `rejected`. */
    readonly serverMessage: string | null;
  }
}

declare function setTimeout(fn: () => void, ms?: number): number;
declare function clearTimeout(id: number): void;
declare function setInterval(fn: () => void, ms: number): number;
declare function clearInterval(id: number): void;

declare const console: {
  debug(...args: unknown[]): void;
  log(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
};
