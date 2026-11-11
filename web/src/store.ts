import { toast } from "solid-sonner";
import { type Accessor, batch, createEffect, createSignal, on, onCleanup } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { type Account, type Action, type Channel, type Ended, type Line, type Session, type Target, api, follow, isOffline } from "./wire";

export const VIEWS = [
  { id: "log", label: "Log" },
  { id: "drops", label: "Drops" },
  { id: "quests", label: "Quests" },
  { id: "inventory", label: "Inventory" },
  { id: "temp", label: "Temp" },
  { id: "bank", label: "Bank" },
  { id: "factions", label: "Factions" },
  { id: "players", label: "Players" },
  { id: "packets", label: "Packets" },
] as const;

export type View = (typeof VIEWS)[number]["id"];

export type GameLines = "shown" | "hidden";

export const isPacket = (channel: Channel) => channel === "sent" || channel === "received";

export const shows = (game: GameLines, channel: Channel) =>
  !isPacket(channel) && (game === "shown" || channel !== "game");

export type ConfigStart = { kind: "list" } | { kind: "account"; user: string } | { kind: "new" };

export type Dialog =
  | { kind: "picker"; queue: boolean }
  | { kind: "run"; script: string; extra: [string, string][]; queue: boolean }
  | { kind: "servers" }
  | { kind: "skua"; script: string }
  | { kind: "queued"; account: number; id: number }
  | { kind: "config"; start: ConfigStart }
  | { kind: "keys" }
  | { kind: "end" }
  | { kind: "restart" }
  | { kind: "confirm"; title: string; body: string; label: string; danger: boolean; run: () => void };


const KEPT_PER_CHANNEL = 1000;

function keepNewest(lines: Line[]): Line[] {
  const counted = new Map<Channel, number>();
  const kept: Line[] = [];
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i];
    if (line === undefined) continue;
    const n = (counted.get(line.channel) ?? 0) + 1;
    counted.set(line.channel, n);
    if (n <= KEPT_PER_CHANNEL) kept.push(line);
  }
  return kept.length === lines.length ? lines : kept.reverse();
}

export function persisted<T extends string>(key: string, allowed: readonly T[], fallback: T): [Accessor<T>, (next: T) => void] {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(key);
  } catch {
    saved = null;
  }
  const [value, setValue] = createSignal<T>(allowed.find((a) => a === saved) ?? fallback);
  const set = (next: T) => {
    setValue(() => next);
    try {
      localStorage.setItem(key, next);
    } catch {
      return;
    }
  };
  return [value, set];
}

const [session, setSession] = createSignal<Session | null>(null);
const [accounts, setAccounts] = createStore<Account[]>([]);

export const [redaction, setRedaction] = persisted<"on" | "off">("aqx-redact", ["on", "off"], "off");

const HOME = /\/(?:Users|home)\/[^/\s]+/g;
const ROOM = /\b([A-Za-z]\w*)-\d+\b/g;
const WORD = /[\p{L}\p{N}_]/u;
const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
let masks: { key: string; pattern: RegExp | null; alias: Map<string, string> } | null = null;

function currentMasks() {
  const pairs = accounts.flatMap((a, i) =>
    [a.name, a.user].filter((s) => s !== "").map((secret): [string, string] => [secret, `account ${i + 1}`]),
  );
  const key = pairs.map(([secret]) => secret).join("\n");
  if (masks?.key === key) return masks;
  const sorted = pairs.sort(([a], [b]) => b.length - a.length);
  const parts = sorted.map(([secret]) => {
    const escaped = escape(secret);
    const before = WORD.test(secret[0] ?? "") ? "(?<![\\p{L}\\p{N}_])" : "";
    const after = WORD.test(secret.at(-1) ?? "") ? "(?![\\p{L}\\p{N}_])" : "";
    return before + escaped + after;
  });
  masks = {
    key,
    pattern: parts.length > 0 ? new RegExp(parts.join("|"), "giu") : null,
    alias: new Map(sorted.map(([secret, alias]) => [secret.toLowerCase(), alias])),
  };
  return masks;
}

export function hide(text: string): string {
  if (redaction() === "off" || !text) return text;
  const { pattern, alias } = currentMasks();
  const homeless = text.replace(HOME, "~");
  const nameless = pattern ? homeless.replace(pattern, (hit) => alias.get(hit.toLowerCase()) ?? hit) : homeless;
  return nameless.replace(ROOM, "$1-•••••");
}
const STAMP = /^\[\d{2}:\d{2}:\d{2}\] /;

export function lineText(line: Line): string {
  return hide(line.channel === "script" ? line.text.replace(STAMP, "") : line.text);
}

export function finder(query: string): RegExp | null {
  return query ? new RegExp(escape(query), "iu") : null;
}

const [log, setLog] = createSignal<Line[]>([]);
const [logLoading, setLogLoading] = createSignal(true);
const [ended, setEnded] = createSignal<Ended | null>(null);
const [connected, setConnected] = createSignal(false);
const [selected, setSelected] = createSignal<number | null>(0);
const [marked, setMarked] = createSignal<ReadonlySet<string>>(new Set());
function perAccount<T extends string>(key: string, allowed: readonly T[], fallback: T): [Accessor<T>, (next: T) => void] {
  let saved: unknown = null;
  try {
    saved = JSON.parse(localStorage.getItem(key) ?? "null");
  } catch {
    saved = null;
  }
  const parsed: Record<string, T> = {};
  if (saved && typeof saved === "object") {
    for (const [name, value] of Object.entries(saved)) {
      const known = allowed.find((a) => a === value);
      if (known) parsed[name] = known;
    }
  }
  const [byAccount, setByAccount] = createSignal(parsed);
  const get = () => {
    const name = current()?.name;
    return (name && byAccount()[name]) || fallback;
  };
  const set = (next: T) => {
    const name = current()?.name;
    if (!name) return;
    const all = { ...byAccount(), [name]: next };
    setByAccount(all);
    try {
      localStorage.setItem(key, JSON.stringify(all));
    } catch {
      return;
    }
  };
  return [get, set];
}

const [view, setView] = perAccount<View>("aqx-views", VIEWS.map((v) => v.id), "log");
const [gameLines, setGameLines] = perAccount<GameLines>("aqx-game-lines", ["shown", "hidden"], "hidden");
const [search, setSearch] = createSignal("");
const [dialog, setDialog] = createSignal<Dialog | null>(null);
const [acting, setActing] = createSignal<number[]>([]);

export {
  accounts,
  connected,
  dialog,
  ended,
  gameLines,
  log,
  logLoading,
  acting,
  marked,
  search,
  selected,
  session,
  setActing,
  setDialog,
  setGameLines,
  setMarked,
  setSearch,
  setView,
  view,
};

export function select(i: number | null) {
  batch(() => {
    setSelected(i);
    setSearch("");
  });
}

export function show(next: View) {
  batch(() => {
    setView(next);
    setSearch("");
  });
}

export function lastLines(user: string): Line[] {
  return ended()?.accounts.find((a) => a.user === user)?.lines ?? [];
}

export function current() {
  const i = selected();
  return i === null ? undefined : accounts[i];
}

export function here(): number[] {
  const i = selected();
  return i === null ? [] : [i];
}

export function chosen(): number[] {
  return accounts.flatMap((a, i) => (marked().has(a.user) ? [i] : []));
}

export function toggleMark(i: number) {
  const user = accounts[i]?.user;
  if (user === undefined) return;
  const next = new Set(marked());
  if (!next.delete(user)) next.add(user);
  setMarked(next);
}

export function markAll() {
  setMarked(chosen().length === accounts.length ? new Set<string>() : new Set(accounts.map((a) => a.user)));
}

export function markWhere(test: (a: Account) => boolean) {
  setMarked(new Set(accounts.filter(test).map((a) => a.user)));
}

export function ordered(): number[] {
  const online: number[] = [];
  const offline: number[] = [];
  accounts.forEach((a, i) => (isOffline(a) ? offline : online).push(i));
  return online.concat(offline);
}

function between(from: number, to: number): number[] {
  const order = ordered();
  const a = order.indexOf(from);
  const b = order.indexOf(to);
  return a < 0 || b < 0 ? [] : order.slice(Math.min(a, b), Math.max(a, b) + 1);
}

export function markRange(from: number, to: number) {
  const next = new Set(marked());
  for (const i of between(from, to)) {
    const user = accounts[i]?.user;
    if (user !== undefined) next.add(user);
  }
  setMarked(next);
}

export function paint(base: ReadonlySet<string>, from: number, to: number, on: boolean) {
  const next = new Set(base);
  for (const i of between(from, to)) {
    const user = accounts[i]?.user;
    if (user === undefined) continue;
    if (on) next.add(user);
    else next.delete(user);
  }
  setMarked(next);
}

export function isMarked(i: number) {
  const user = accounts[i]?.user;
  return user !== undefined && marked().has(user);
}

export function targetsOf(on: number[]): Target[] {
  return on.flatMap((account) => {
    const name = accounts[account]?.name;
    return name === undefined ? [] : [{ account, name }];
  });
}

export function startedMs(): number {
  const s = session();
  return s?.kind === "running" || s?.kind === "incompatible" ? s.startedMs : 0;
}

export function say(text: string) {
  if (text) toast(hide(text));
}

export function fail(text: string) {
  if (text) toast.error(hide(text), { duration: 8000 });
}

export function nameOf(i: number) {
  return hide(accounts[i]?.name ?? `account ${i + 1}`);
}

export function report(outcomes: { account: number; error: string | null }[], done: string) {
  const sent = outcomes.filter((o) => o.error === null).map((o) => nameOf(o.account));
  const parts = sent.length ? [`${sent.join(", ")}: ${done}`] : [];
  const reasons = new Map<string, string[]>();
  for (const o of outcomes) {
    if (o.error !== null) reasons.set(o.error, [...(reasons.get(o.error) ?? []), nameOf(o.account)]);
  }
  if (parts.length) say(parts.join("; "));
  for (const [why, names] of reasons) fail(`${names.join(", ")}: ${why}`);
}

export async function act(action: Action, done: string, on: number[]) {
  if (on.length === 0) return;
  try {
    report(await api.act(targetsOf(on), action), done);
  } catch (e) {
    fail(e instanceof Error ? e.message : String(e));
  }
}

export function startFeed() {
  createEffect(
    on(selected, (i) => {
      setLog([]);
      setLogLoading(true);
      let started: number | null = null;
      const stop = follow(i, {
        session: (next) => {
          const startedMs = next.kind === "running" ? next.startedMs : null;
          if (startedMs !== null && started !== null && startedMs !== started) setLog([]);
          if (startedMs !== null) started = startedMs;
          setSession(next);
        },
        accounts: (all) => {
          setAccounts(reconcile(all));
          setLogLoading(false);
          if (selected() !== null && (selected() ?? 0) >= all.length) setSelected(all.length ? 0 : null);
        },
        account: (at, account) => setAccounts(at, reconcile(account)),
        log: (lines) =>
          setLog((kept) => keepNewest(kept.concat(lines))),
        ended: setEnded,
        connected: setConnected,
      });
      onCleanup(stop);
    }),
  );
}
