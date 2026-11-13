export type Step = "Connecting" | "LoggingIn" | "Joining" | "Loading";

export type State =
  | { Offline: { why: string | null } }
  | { Busy: Step }
  | { Reconnecting: string }
  | "Online"
  | "Running"
  | "Paused"
  | "Stalled";

export type ItemCategory = "classes" | "weapons" | "armor" | "helms" | "capes" | "pets" | "accessories" | "items" | "consumables" | "enhancements" | "other";

export type Stack = {
  id: number;
  name: string;
  qty: number;
  max: number;
  equipped: boolean;
  rank: number | null;
  category: ItemCategory;
};

export type Aura = {
  name: string;
  stacks: number;
  description: string;
  timer: { duration_ms: number; left_ms: number } | null;
};

export type Game = {
  area: string;
  cell: string;
  pad: string;
  hp: number;
  max_hp: number;
  mp: number;
  max_mp: number;
  dead: boolean;
  away: boolean;
  member: boolean;
  class: string;
  class_rank: number;
  level: number;
  exp: number;
  exp_to_level: number;
  gold: number;
  skills: { name: string; ready: number; left_ms: number }[];
  auras: Aura[];
  target: { id: number; name: string; hp: number; max_hp: number; auras: Aura[] } | null;
  players: { name: string; level: number | null }[];
  drops: Stack[];
  quests: { id: number; name: string; items: { name: string; have: number; need: number }[] }[];
  factions: { name: string; rank: number; rep: number; next: number | null; gained: boolean }[];
  temp: Stack[];
  inventory: Stack[];
  bag_used: number;
  misc_used: number;
  bag_slots: number;
  bank: Stack[] | null;
  bank_used: number;
  bank_slots: number;
};

export type Item = number | string;

export type RegisteredQuest = { id: number; reward?: Item };

export type Registered = { drops: Item[]; quests: RegisteredQuest[] };

export type Edit =
  | { register_drop: Item }
  | { unregister_drop: Item }
  | { register_quest: RegisteredQuest }
  | { unregister_quest: number };

type AccountData = {
  name: string;
  user: string;
  state: State;
  server: string;
  script: string | null;
  status: string;
  registered: Registered | null;
  up: { secs: number; nanos: number } | null;
  stats: { kills: number; deaths: number; drops: number; gold_from: number | null };
  gold_gained: number | null;
  game: Game | null;
  recording: string | null;
  queue: Queued[];
};

export type Account = AccountData & { sampledAt: number };

export type Channel = "aqx" | "script" | "server" | "chat" | "game" | "sent" | "received";

export type Line = { seq: number; at_ms: number; channel: Channel; text: string };

export type Session =
  | { kind: "none" }
  | { kind: "running"; startedMs: number; otherBuild: boolean; configChanged: boolean }
  | { kind: "incompatible"; startedMs: number }
  | { kind: "ending" };

export type Ended = { endedMs: number; accounts: { name: string; user: string; lines: Line[] }[] };

export type Script = { path: string; args: [string, string][] };

export type Queued = { id: number; script: Script };

export type Action =
  | "login"
  | "login_only"
  | "logout"
  | "stop"
  | "restart"
  | "pause"
  | "resume"
  | "relogin"
  | { switch: string }
  | { run: Script }
  | { enqueue: Script }
  | { unqueue: number }
  | { move_queued: { id: number; to: number } }
  | { set_queued_args: { id: number; args: [string, string][] } }
  | "clear_queue"
  | "next"
  | { pick_up: number[] }
  | { registry: Edit }
  | { record: null }
  | "stop_recording";

export type Outcome = { account: number; error: string | null };

export type Target = { account: number; name: string };

export type Server = {
  name: string;
  players: number;
  capacity: number;
  members: boolean;
  online: boolean;
};

export type Folder = {
  dir: string;
  shown: string;
  parent: string | null;
  entries: { name: string; dir: boolean }[];
};

export type SkuaLibrary =
  | { state: "missing"; folder: string }
  | { state: "failed"; error: string }
  | {
      state: "ready";
      commit: string;
      folder: string;
      entries: { name: string; description: string; tags: string[]; path: string }[];
    };

export type ArgKind = "string" | "number" | "boolean" | "list" | "map";

export type DeclaredArg = {
  name: string;
  kind: ArgKind;
  optional: boolean;
  default: string | null;
  shared: string | null;
  each: (string | null)[];
};

export type Declared = { path: string; shown: string; args: DeclaredArg[] };

export type Problem = { account: number; arg: string; error: string };

export type Recent = { path: string; shown: string; args: [string, string][] };

export type SkuaOption = {
  group: string;
  name: string;
  label: string;
  description: string;
  kind: "bool" | "number" | "text" | "choice";
  choices: string[];
  value: string | null;
  default: string | null;
};

export type SkuaPane = { error: string } | { storage: string | null; options: SkuaOption[] };

export type JsonValue = string | number | boolean | JsonValue[] | { [key: string]: JsonValue };

export type Fields = Record<string, JsonValue>;

export type ConfigForm = {
  defaults: Fields;
  accounts: { fields: Fields; hasPassword: boolean }[];
};

export type SchemaProperty = {
  description?: string;
  type?: string | string[];
  anyOf?: { $ref?: string; type?: string }[];
  pattern?: string;
};

export type ConfigSchema = {
  Settings: { properties: Record<string, SchemaProperty> };
  AccountConfig: { properties: Record<string, SchemaProperty>; required?: string[] };
  OnStall: { oneOf: { const: string; description: string }[] };
};

export type ConfigFile = {
  path: string;
  version: string;
  text: string;
  form: ConfigForm;
  schema: ConfigSchema;
  clientVersion: string;
};

export type EditedAccount = { from: number | null; fields: Fields; password: string | null };

export type { CustomHotkeys as CustomKeys } from "@tanstack/solid-hotkeys";

import type { CustomHotkeys } from "@tanstack/solid-hotkeys";

export class Refused extends Error {}

function decode<T>(text: string): T {
  return JSON.parse(text);
}

async function call<T>(path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  if (!response.ok && response.status !== 422) {
    const error = text.startsWith("{") ? decode<{ error?: string }>(text).error : undefined;
    throw new Refused(error ?? `${response.status} ${response.statusText}`);
  }
  return decode<T>(text);
}

const q = (params: Record<string, string>) => new URLSearchParams(params).toString();

export const api = {
  act: (on: Target[], action: Action) =>
    call<Outcome[]>("/api/act", { accounts: on.map((t) => t.account), names: on.map((t) => t.name), action }),
  run: (body: {
    script: string;
    extra: [string, string][];
    runs: { account: number; name: string; args: [string, string][] }[];
    shared: boolean;
    queue: boolean;
  }) => call<Outcome[] | { problems: Problem[] }>("/api/run", body),
  args: (script: string, accounts: number[]) =>
    call<Declared>(`/api/args?${q({ script, accounts: accounts.join(",") })}`),
  recent: () => call<{ runs: Recent[]; hidden: string[] }>("/api/recent"),
  forgetRecent: (path: string) => call<{ runs: Recent[]; hidden: string[] }>("/api/recent/forget", { path }),
  servers: () => call<Server[]>("/api/servers/all"),
  files: (dir: string) => call<Folder>(`/api/files?${q({ dir })}`),
  skua: (query: string) => call<SkuaLibrary>(`/api/skua?${q({ q: query })}`),
  updateSkua: () => call<{ message: string }>("/api/skua/update", {}),
  skuaOptions: (account: number, name: string, script: string) =>
    call<{ corebots: SkuaPane; script: SkuaPane }>(
      `/api/skua/options?${q({ account: String(account), name, script })}`,
    ),
  skuaScriptOptions: (script: string) => call<SkuaPane>(`/api/skua/script-options?${q({ script })}`),
  saveSkuaOptions: (body: {
    account: number;
    name: string;
    script: string;
    tab: "corebots" | "script";
    changes: [string, string][];
  }) => call<SkuaPane>("/api/skua/options", body),
  config: () => call<ConfigFile>("/api/config"),
  saveConfig: (
    version: string,
    edit: { text: string } | { form: { defaults: Fields; accounts: EditedAccount[] } },
  ) => call<ConfigFile>("/api/config", { version, ...edit }),
  keys: () => call<CustomHotkeys>("/api/keys"),
  saveKeys: (keys: CustomHotkeys) => call<CustomHotkeys>("/api/keys", keys),
  endSession: (startedMs: number) => call<{ ended: true }>("/api/session/end", { startedMs }),
  restartSession: (startedMs: number) =>
    call<{ restarted: boolean; gone?: number }>("/api/session/restart", { startedMs }),
};

export type Feed = {
  session: (session: Session) => void;
  accounts: (accounts: Account[]) => void;
  account: (i: number, account: Account) => void;
  log: (lines: Line[]) => void;
  ended: (ended: Ended) => void;
  connected: (up: boolean) => void;
};

const receivedAccount = (account: AccountData): Account => ({ ...account, sampledAt: performance.now() });

export function follow(selected: number | null, feed: Feed): () => void {
  let resume = "";
  const url = () => {
    const params: Record<string, string> = {};
    if (selected !== null) params.account = String(selected);
    if (resume) params.resume = resume;
    return `/api/events?${q(params)}`;
  };
  let source: EventSource | null = null;
  let retry: number | undefined;
  let wait = 1000;
  let stopped = false;
  const open = () => {
    clearTimeout(retry);
    source?.close();
    source = null;
    if (document.visibilityState !== "visible" || stopped) return;
    const next = new EventSource(url());
    source = next;
    next.addEventListener("open", () => {
      wait = 1000;
      feed.connected(true);
    });
    next.addEventListener("error", () => {
      if (source !== next || document.visibilityState !== "visible" || stopped) return;
      feed.connected(false);
      if (next.readyState === EventSource.CLOSED && !stopped) {
        retry = window.setTimeout(open, wait);
        wait = Math.min(wait * 2, 15000);
      }
    });
    next.addEventListener("session", (e) => feed.session(decode<Session>(e.data)));
    next.addEventListener("accounts", (e) => feed.accounts(decode<AccountData[]>(e.data).map(receivedAccount)));
    next.addEventListener("account", (e) => {
      const { i, account } = decode<{ i: number; account: AccountData }>(e.data);
      feed.account(i, receivedAccount(account));
    });
    next.addEventListener("ended", (e) => feed.ended(decode<Ended>(e.data)));
    next.addEventListener("log", (e) => {
      if (e.lastEventId) resume = e.lastEventId;
      feed.log(decode<Line[]>(e.data));
    });
  };
  const back = () => {
    if (document.visibilityState !== "visible") {
      clearTimeout(retry);
      source?.close();
      source = null;
    } else if (!source || source.readyState === EventSource.CLOSED) {
      wait = 1000;
      open();
    }
  };
  document.addEventListener("visibilitychange", back);
  open();
  return () => {
    stopped = true;
    clearTimeout(retry);
    document.removeEventListener("visibilitychange", back);
    source?.close();
  };
}

export function stateWord(state: State): string {
  if (typeof state === "string") return state.toLowerCase();
  if ("Offline" in state) return "offline";
  if ("Busy" in state) {
    const steps: Record<Step, string> = {
      Connecting: "connecting",
      LoggingIn: "logging in",
      Joining: "joining",
      Loading: "loading",
    };
    return steps[state.Busy];
  }
  return "reconnecting";
}

export const isOffline = (a: Account) => typeof a.state === "object" && "Offline" in a.state;
export const isRunning = (a: Account) =>
  a.state === "Running" || a.state === "Paused" || a.state === "Stalled";
export const isOnline = (a: Account) =>
  a.state === "Online" || isRunning(a);

export function reason(a: Account): string | null {
  const s = a.state;
  if (typeof s === "object" && "Offline" in s) return s.Offline.why;
  if (typeof s === "object" && "Reconnecting" in s) return s.Reconnecting;
  return null;
}

export function stateLabel(a: Account): string {
  const s = a.state;
  if (typeof s === "object" && "Offline" in s && s.Offline.why) return "failed";
  return stateWord(s);
}
