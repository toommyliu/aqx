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

  /** 0 = auto attack, 1-4 = class skills, 5 = the consumable equipped with `inventory.equip`. */
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
    readonly attackPower: number;
    readonly spellPower: number;
    /** Damage of a critical hit as a multiple of a normal one, e.g. 1.5. */
    readonly critMultiplier: number;
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
    readonly state: State;
    readonly auras: readonly Aura[];
  }

  export interface OtherPlayer {
    /** The entity id that raw packets use for this player, as in `p:{id}`. */
    readonly id: number;
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
    /** The server's id for this copy of the item, `null` for temp items. */
    readonly charItem: number | null;
    readonly name: string;
    readonly qty: number;
    readonly maxStack: number;
    /** Server `sType`, e.g. "Class", "Sword", "Item", "Quest Item". */
    readonly type: string;
    /** Server `sES`, the equipment slot, e.g. "Weapon", "ar", "he", "None". */
    readonly slot: string;
    readonly level: number;
    readonly link: string;
    readonly file: string;
    readonly icon: string;
    readonly meta: string;
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
    /** The item only lasts until logout, like most kill drops. */
    readonly temp: boolean;
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
    /** The class the quest needs and the class points it needs in it, `null` for most quests. A quest stays locked until then. */
    readonly class: { readonly item: number; readonly points: number } | null;
    readonly gold: number;
    readonly experience: number;
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
    /** Fight in this cell. Without it, the player hunts across cells for live targets. */
    readonly cell?: string;
    /** Default `"Left"`. */
    readonly pad?: string;
    /** Keep killing matching monsters until this returns true. It is checked continuously, so the fight stops as soon as it holds. */
    readonly until?: () => boolean;
    readonly timeout?: number;
  }

  export interface FarmOptions extends Fight {
    /** Without it, the player fights any monster in the cell and pad it stands on when `farm` starts, and never leaves. */
    readonly target?: Target;
    readonly cell?: string;
    readonly pad?: string;
    readonly timeout?: number;
  }

  export interface EventMap {
    death: undefined;
    respawn: undefined;
    map: { readonly name: string; readonly room: number };
    /** The player's cell or pad changed: a jump, a respawn, or the server moving it. */
    cell: { readonly cell: string; readonly pad: string };
    kill: Monster;
    drop: Drop;
    /** `qty` is the new total in that place, 0 once the item is gone. */
    item: { readonly name: string; readonly qty: number; readonly where: "inventory" | "house" | "temp" | "bank" };
    quest: { readonly id: number; readonly change: "accepted" | "completed" };
    chat: {
      readonly from: string;
      readonly text: string;
      readonly channel: "zone" | "party" | "guild" | "whisper";
    };
    warning: { readonly text: string };
    /**
     * An aura the server added to or removed from the player (`monster: null`)
     * or a monster. Auras that run out or end with a death raise no event.
     */
    aura: {
      readonly name: string;
      readonly stacks: number;
      readonly change: "added" | "removed";
      readonly monster: number | null;
    };
    /** A monster started telegraphing a counter attack, or its "Counter Attack" aura ended. */
    counter: { readonly monster: number; readonly active: boolean };
    /** The server marked the player away, or back. */
    afk: { readonly afk: boolean };
  }

  export interface Packet {
    readonly cmd: string;
    /** The `b.o` object of a JSON message, `null` for the other kinds. */
    readonly data: Readonly<Record<string, any>> | null;
    /** The fields of an XT string after its room, empty for the other kinds. */
    readonly args: readonly string[];
    /** The message as it arrived, after decompression. */
    readonly raw: string;
  }

  export interface Server {
    readonly name: string;
    readonly online: boolean;
    readonly members: boolean;
    readonly players: number;
    readonly capacity: number;
  }

  export const player: {
    /** The entity id that raw packets use for the player, as in `p:{id}`. */
    readonly id: number;
    /** The character id, as account.aq.com takes it. `null` until login sends it. */
    readonly charId: number | null;
    readonly name: string;
    readonly level: number;
    readonly hp: number;
    readonly maxHp: number;
    readonly mp: number;
    readonly maxMp: number;
    readonly state: State;
    readonly cell: string;
    readonly pad: string;
    /** Where the player last walked to in the cell, or where the server placed it on joining. 0 after a jump. */
    readonly x: number;
    readonly y: number;
    /** The server marked the player away. The client answers that on its own. */
    readonly afk: boolean;
    /** The monster a `kill` or `farm` is fighting right now. */
    readonly target: Monster | null;
    readonly gold: number;
    /** Adventure Coins. */
    readonly coins: number;
    readonly member: boolean;
    /** The account has an AdventureQuest character linked. */
    readonly aqLinked: boolean;
    readonly emailVerified: boolean;
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
    /** The server's id for this map instance, which raw packets carry as their room. */
    readonly areaId: number;
    /** The cells in the map file, in its order. Empty until the file loads, and for a file aqx can't read. */
    readonly cells: readonly string[];
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
    /** Walks to a point in the current cell. Returns once sent; the server doesn't acknowledge it. */
    walk(x: number, y: number): void;
    /** Goes to another player's map and cell. Throws `timeout` when the server doesn't move the player there, as when the other player is offline. */
    goto(player: string): void;
    /** Picks up a map item until the inventories hold `qty` of `item`. Throws `rejected` after 3 tries that give nothing. */
    collect(mapItem: number, item: ItemRef, qty?: number): void;
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
    /**
     * Pulls the matching monsters in the player's cell every 500 ms until
     * called with `null` or the script ends. The real client never sends this.
     */
    aggro(target: Target | null): void;
    /** Leaves combat by jumping to the current cell again. */
    exit(): void;
  };

  export const skills: {
    readonly slots: readonly Skill[];
    /**
     * Casts one slot as soon as its cooldown, the global cooldown and mana
     * allow, and returns once it's sent. Throws `timeout` when mana falls
     * short for 30 s. Hostile skills hit the matching monster in the cell
     * with the lowest HP; others target the player.
     */
    use(slot: Slot, target?: Target): void;
  };

  export const inventory: Items & {
    readonly temp: Items;
    readonly slots: number;
    readonly used: number;
    /** Item, Note, Quest Item and Resource stacks, which fill their own pool instead of the bag. */
    readonly misc: { readonly slots: number; readonly used: number };
    /** Withdraws from the bank first if needed. */
    equip(item: ItemRef): void;
    /** Uses a "ServerUse" item such as a boost. Returns once sent. */
    use(item: ItemRef): void;
  };

  /** Houses, floor and wall items, which the server keeps apart from the inventory. */
  export const house: Items & {
    readonly slots: number;
    readonly used: number;
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

  /** The escape hatch. Packets sent here go through the same pacer as everything else. */
  export const packet: {
    /** A raw packet such as `%xt%zm%cmd%${map.areaId}%...%`, without the trailing NUL. */
    send(text: string): void;
    /** Runs for every inbound message with this cmd: the XT or JSON `cmd`, or the XML `action`. */
    on(cmd: string, handler: (p: Packet) => void): () => void;
    /** Sends `text` and returns the next `cmd` message that passes `filter`. Throws `timeout` after `timeout` ms, default 10000. */
    request(text: string, cmd: string, opts?: { readonly timeout?: number; readonly filter?: (p: Packet) => boolean }): Packet;
  };

  export const session: {
    /** The server the account is on. */
    readonly server: string;
    servers(): Server[];
    /**
     * Logs out and back in, to `server` if given, else to the current one.
     * Returns once the character has loaded. The game state starts over: the
     * player is back in its home map, and the bank and shops load again.
     */
    relogin(server?: string): void;
  };

  export const wait: {
    /** Re-evaluates `predicate` after every state change and at least every 250 ms. Returns `false` on timeout. */
    until(predicate: () => boolean, timeout?: number): boolean;
    ms(ms: number): void;
  };

  export type ArgDefault = string | number | boolean | StringConstructor | NumberConstructor | BooleanConstructor;
  export type ArgValue<D> = D extends StringConstructor ? string : D extends NumberConstructor ? number : D extends BooleanConstructor ? boolean : D;

  export const script: {
    /**
     * Script inputs. Each key's default fixes its type; `--arg key=value` on
     * the CLI overrides it. `String`, `Number` or `Boolean` in place of a default
     * makes the arg required. Before running a script, the TUI asks for each arg
     * that wasn't typed, offering the default; a required one needs a value.
     * Throws on unknown keys, missing required args, or values that don't parse
     * as the default's type.
     */
    args<T extends Record<string, ArgDefault>>(defaults: T): { [K in keyof T]: ArgValue<T[K]> };
    /** One line describing what the script is doing. Shown in the TUI and logs. */
    status(text: string): void;
    /** Named progress bar with rate and ETA. `current` is absolute, not a delta. */
    progress(label: string, current: number, total?: number): void;
    /** Ends the script. Catching the error it throws does not keep the script alive. */
    stop(reason?: string): never;
  };

  /** Reverts to the account's defaults when the script ends. */
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

/** Timers fire while the script waits inside a blocking call, like event handlers. They end with the script. */
declare function setTimeout<A extends unknown[]>(handler: (...args: A) => void, ms?: number, ...args: A): number;
declare function setInterval<A extends unknown[]>(handler: (...args: A) => void, ms?: number, ...args: A): number;
declare function clearTimeout(id: number | undefined): void;
declare function clearInterval(id: number | undefined): void;

declare const console: {
  debug(...args: unknown[]): void;
  log(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
};
