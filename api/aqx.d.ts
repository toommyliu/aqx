/*
 * Every function in this module blocks until it is done. While a call blocks,
 * the client keeps reading the socket, fighting and running event handlers.
 * Predicates (`when`, `hold`, `until`, `wait.until`, function `Target`s) must not call
 * anything that blocks; doing so throws. A predicate that throws aborts the
 * blocking call it runs under with that error, and so does an event handler.
 *
 * Everything a script sets up (handlers, registered quests and drops,
 * settings) ends with the script.
 */
declare module "aqx" {
  /** Item name (case-insensitive, exact) or ItemID. */
  export type ItemRef = string | number;

  /** Monster name (case-insensitive, `"*"` = any), MonMapID, a list of those, or a predicate. */
  export type Target = string | number | readonly (string | number)[] | ((m: Monster) => boolean);

  /** 0 = auto attack, 1-4 = class skills, 5 = equipped consumable, which a fight can't use yet. */
  export type Slot = 0 | 1 | 2 | 3 | 4 | 5;

  export type State = "dead" | "idle" | "combat";

  /** `cancelled`: the script is being stopped or the connection closed. */
  export type ErrorCode = "timeout" | "rejected" | "not_found" | "cancelled";

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
    /** `Date.now()` timestamp, `null` for auras without a duration. */
    readonly expiresAt: number | null;
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
  }

  export interface Skill {
    readonly slot: Slot;
    readonly name: string;
    readonly mana: number;
    /** Effective cooldown in ms after haste. */
    readonly cooldown: number;
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

  export type Step = Exclude<Slot, 5> | { readonly slot: Exclude<Slot, 5>; readonly when: () => boolean };

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
    /** Default `"Left"`. */
    readonly pad?: string;
    /** Keep killing matching monsters until this returns true. It is checked continuously, so the fight stops as soon as it holds. */
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

  export interface EventMap {
    death: undefined;
    respawn: undefined;
    map: { readonly name: string; readonly room: number };
    kill: Monster;
    drop: Drop;
    /** `qty` is the new total in that place, 0 once the item is gone. */
    item: { readonly name: string; readonly qty: number; readonly where: "inventory" | "temp" | "bank" };
    quest: { readonly id: number; readonly change: "accepted" | "completed" };
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
    readonly cell: string;
    readonly pad: string;
    readonly gold: number;
    readonly member: boolean;
    /** Name of the equipped class, `""` before the server sends it. */
    readonly class: string;
    readonly stats: Stats;
    readonly auras: readonly Aura[];
    readonly factions: readonly Faction[];
    /** Returns when HP and MP are full. Throws `timeout` after 30 s. */
    rest(): void;
  };

  export const map: {
    readonly name: string;
    readonly room: number;
    readonly monsters: readonly Monster[];
    readonly players: readonly OtherPlayer[];
    find(target: Target): Monster[];
    /**
     * `name` may carry a room suffix, as in `join("timespace-12345")`. Without
     * one, `settings.room` is used. No-op if already there. Returns once the
     * player stands in `cell`. `pad` defaults to `"Left"`.
     */
    join(name: string, cell?: string, pad?: string): void;
    jump(cell: string, pad?: string): void;
  };

  export const combat: {
    /** Returns after one matching monster dies, or, with `opts.until`, once that returns true. */
    kill(target: Target, opts?: KillOptions): void;
    /**
     * Withdraws the items from the bank, picks up their drops, shows a progress
     * bar per item, and kills until every count is reached.
     */
    farm(item: ItemRef, qty: number, opts?: FarmOptions): void;
    farm(goals: readonly Goal[], opts?: FarmOptions): void;
  };

  export const skills: {
    readonly slots: readonly Skill[];
  };

  export const inventory: Items & {
    readonly temp: Items;
    readonly slots: number;
    readonly used: number;
    /** Withdraws from the bank first if needed. */
    equip(item: ItemRef): void;
  };

  /** Reading `items` loads the bank on first use, except inside a predicate. */
  export const bank: Items & {
    readonly slots: number;
    readonly used: number;
    /** Skips items already in the inventory. */
    withdraw(...items: ItemRef[]): void;
    /** Skips items already in the bank. */
    deposit(...items: ItemRef[]): void;
  };

  export const quests: {
    /** Loads the quest on first call. Later calls, and calls inside predicates, return its current status without blocking. */
    get(id: number): Quest;
    /** Skips quests already active. */
    accept(...ids: number[]): void;
    /** `times` turns in that many completions at once when the quest allows it. */
    complete(id: number, opts?: { readonly reward?: number; readonly times?: number }): void;
    readonly registered: readonly number[];
    /**
     * Accepts, completes and re-accepts in the background whenever an item
     * changes. Registering a quest again replaces its reward. A quest that
     * fails to accept or complete is logged and unregistered.
     */
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
    /**
     * Handlers run while the script is inside a blocking call, and the events
     * a call causes reach them before that call returns. Events that arrive
     * during a handler queue until it returns.
     */
    on<K extends keyof EventMap>(event: K, handler: (e: EventMap[K]) => void): () => void;
    /** Throws `timeout` if `opts.timeout` elapses first. */
    once<K extends keyof EventMap>(
      event: K,
      opts?: { readonly timeout?: number; readonly filter?: (e: EventMap[K]) => boolean },
    ): EventMap[K];
  };

  export const wait: {
    /** Re-evaluates `predicate` after every state change and at least every 250 ms. Returns `false` on timeout. */
    until(predicate: () => boolean, timeout?: number): boolean;
    ms(ms: number): void;
  };

  export const script: {
    /**
     * Script inputs. Each key's default fixes its type; `--arg key=value` on
     * the CLI overrides it. Throws on unknown keys or values that don't parse
     * as the default's type.
     */
    args<T extends Record<string, string | number | boolean>>(defaults: T): T;
    /** One line describing what the script is doing. Shown in the TUI and logs. */
    status(text: string): void;
    /** Named progress bar with rate and ETA. `current` is absolute, not a delta. */
    progress(label: string, current: number, total?: number): void;
    /** Ends the script. Catching the error it throws does not keep the script alive. */
    stop(reason?: string): never;
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
  }
}

declare const console: {
  debug(...args: unknown[]): void;
  log(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
};
