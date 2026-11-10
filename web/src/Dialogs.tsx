import { createDebouncedValue } from "@tanstack/solid-pacer";
import { createMutation, createQuery, keepPreviousData, useQueryClient } from "@tanstack/solid-query";
import { For, Match, Show, Switch, createEffect, createMemo, createSignal, lazy, on, onMount } from "solid-js";

const Config = lazy(() => import("./Config").then((m) => ({ default: m.Config })));
const Keys = lazy(() => import("./Keys").then((m) => ({ default: m.Keys })));
import {
  accounts,
  act,
  acting,
  dialog,
  fail,
  nameOf,
  report,
  say,
  selected,
  setActing,
  persisted,
  setDialog,
  startedMs,
  targetsOf,
  hide,
} from "./store";
import { Icon, Modal, VirtualList, arrows } from "./ui";
import {
  type DeclaredArg,
  type Problem,
  type SkuaOption,
  type SkuaPane,
  api,
  isOffline,
  isRunning,
} from "./wire";

const close = () => setDialog(null);
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function openPicker(on: number[]) {
  if (on.length === 0) return;
  setActing(on);
  setDialog({ kind: "picker" });
}

export function logOut(on: number[]) {
  if (on.length === 0) return;
  const one = on.length === 1;
  setDialog({
    kind: "confirm",
    title: one ? `Log out ${nameOf(on[0] ?? 0)}?` : `Log out ${on.length} accounts?`,
    body: one ? "Its script stops and it logs out." : "Their scripts stop and they log out.",
    label: "Log out",
    danger: true,
    run: () => void act("logout", "logging out", on),
  });
}

export function openServers(on: number[]) {
  if (on.length === 0) return;
  setActing(on);
  setDialog({ kind: "servers" });
}

const shownName = (path: string) => path.split("/").pop() ?? path;

function runEffect(on: number[]): { body: string; danger: boolean } {
  const offline = on.filter((i) => accounts[i] !== undefined && isOffline(accounts[i])).length;
  const running = on.filter((i) => accounts[i] !== undefined && isRunning(accounts[i])).length;
  const first: string[] = [];
  if (offline > 0) first.push(offline === 1 ? "1 offline account logs in first." : `${offline} offline accounts log in first.`);
  if (running > 0)
    first.push(running === 1 ? "1 account stops the script it's running." : `${running} accounts stop the scripts they're running.`);
  const body = [...first, first.length > 0 ? "Then each starts this script." : "Each starts this script."].join(" ");
  return { body, danger: running > 0 };
}

let starting = false;

async function runNow(script: string, extra: [string, string][]) {
  if (starting) return;
  starting = true;
  const on = acting();
  try {
    const declared = await api.args(script, on);
    if (declared.args.length > 0) {
      setDialog({ kind: "run", script, extra });
      return;
    }
    const start = async () => {
      const outcome = await api.run({
        script,
        extra,
        runs: targetsOf(on).map((t) => ({ ...t, args: [] })),
        shared: true,
      });
      if (Array.isArray(outcome)) report(outcome, `running ${shownName(script)}`);
    };
    if (on.length > 1) {
      setDialog({
        kind: "confirm",
        title: `Run ${shownName(script)} on ${on.length} accounts?`,
        ...runEffect(on),
        label: "Run",
        run: () => void start().catch((e) => fail(message(e))),
      });
      return;
    }
    close();
    await start();
  } catch (e) {
    fail(message(e));
  } finally {
    starting = false;
  }
}

const straight = (text: string) => text.replace(/[\u201C\u201D]/g, '"').replace(/[\u2018\u2019]/g, "'");

function typedRun(typed: string): { script: string; extra: [string, string][] } | string {
  const [script = "", ...words] = straight(typed).trim().split(/\s+/);
  const extra: [string, string][] = [];
  for (const word of words) {
    const at = word.indexOf("=");
    const last = extra[extra.length - 1];
    if (at > 0) extra.push([word.slice(0, at), word.slice(at + 1)]);
    else if (last) last[1] += ` ${word}`;
    else return `${word} isn't KEY=VALUE`;
  }
  return { script, extra };
}

export function Dialogs() {
  let opener: Element | null = null;
  createEffect(
    on(dialog, (now, before) => {
      if (now && !before) opener = document.activeElement;
      if (!now && before && opener instanceof HTMLElement) opener.focus();
    }),
  );
  return (
    <Switch>
      <Match when={dialog()?.kind === "picker"}>
        <Picker />
      </Match>
      <Match when={(() => { const d = dialog(); return d?.kind === "run" ? d : undefined; })()}>
        {(d) => <RunArgs script={d().script} extra={d().extra} />}
      </Match>
      <Match when={dialog()?.kind === "servers"}>
        <Servers />
      </Match>
      <Match when={(() => { const d = dialog(); return d?.kind === "skua" ? d : undefined; })()}>
        {(d) => <SkuaOptions script={d().script} />}
      </Match>
      <Match when={(() => { const d = dialog(); return d?.kind === "config" ? d : undefined; })()}>
        {(d) => <Config start={d().start} />}
      </Match>
      <Match when={dialog()?.kind === "keys"}>
        <Keys />
      </Match>
      <Match when={dialog()?.kind === "end"}>
        <End />
      </Match>
      <Match when={dialog()?.kind === "restart"}>
        <Restart />
      </Match>
      <Match when={(() => { const d = dialog(); return d?.kind === "confirm" ? d : undefined; })()}>
        {(d) => <Confirm {...d()} />}
      </Match>
    </Switch>
  );
}

type PickerTab = "files" | "skua" | "recent";
const [pickerTab, setPickerTab] = persisted<PickerTab>("aqx-picker-tab", ["recent", "skua", "files"], "recent");
const optionsTab = persisted<"corebots" | "script">("aqx-options-tab", ["corebots", "script"], "corebots");
const touch = () => !matchMedia("(pointer: fine)").matches;
const focusUnlessTouch = (el: HTMLElement | null | undefined) => {
  if (!touch()) el?.focus();
};
const folderOf = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf("/")));
const [lastDir, setLastDir] = createSignal("");

type Pick = { onPick: (path: string, extra: [string, string][]) => void; options: boolean; focus?: boolean };

export function Picker() {
  const who = () => {
    const on = acting();
    return on.length === 1 ? nameOf(on[0] ?? 0) : `${on.length} accounts`;
  };
  return (
    <Modal title={`Run a script on ${who()}`} onClose={close} wide>
      <ScriptPicker />
    </Modal>
  );
}

export function ScriptPicker(props: { pick?: Pick }) {
  const opened = pickerTab();
  const [switched, setSwitched] = createSignal(false);
  const pick = (): Pick => ({
    ...(props.pick ?? { onPick: (path, extra) => void runNow(path, extra), options: true }),
    focus: !switched() && pickerTab() === opened,
  });
  const tabs: { id: PickerTab; label: string }[] = [
    { id: "recent", label: "Recent" },
    { id: "files", label: "Files" },
    { id: "skua", label: "Skua" },
  ];
  let tablist!: HTMLDivElement;
  return (
    <>
      <div
        class="tabs"
        role="tablist"
        aria-label="Script source"
        ref={tablist}
        onKeyDown={(e) => {
          const at = tabs.findIndex((t) => t.id === pickerTab());
          const by = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
          if (!by) return;
          e.preventDefault();
          const next = tabs[(at + by + tabs.length) % tabs.length];
          if (!next) return;
          setSwitched(true);
          setPickerTab(next.id);
          tablist.querySelector<HTMLElement>(`#pick-${next.id}`)?.focus();
        }}
      >
        <For each={tabs}>
          {(t) => (
            <button
              type="button"
              role="tab"
              id={`pick-${t.id}`}
              aria-selected={pickerTab() === t.id}
              aria-controls="pick-panel"
              tabindex={pickerTab() === t.id ? 0 : -1}
              onClick={() => {
                setSwitched(true);
                setPickerTab(t.id);
              }}
            >
              {t.label}
            </button>
          )}
        </For>
      </div>
      <div id="pick-panel" role="tabpanel" aria-labelledby={`pick-${pickerTab()}`}>
        <Switch>
          <Match when={pickerTab() === "files"}>
            <Files {...pick()} />
          </Match>
          <Match when={pickerTab() === "skua"}>
            <Skua {...pick()} />
          </Match>
          <Match when={pickerTab() === "recent"}>
            <RecentRuns {...pick()} />
          </Match>
        </Switch>
      </div>
    </>
  );
}

function Files(props: Pick) {
  const own = accounts[acting()[0] ?? selected() ?? 0]?.script;
  const [dir, setDir] = createSignal(lastDir() || (own && !own.endsWith(".cs") ? folderOf(own) : ""));
  const [typed, setTyped] = createSignal("");
  const listing = createQuery(() => ({ queryKey: ["files", dir()], queryFn: () => api.files(dir()) }));
  const folder = () => listing.data;
  const [error, setError] = createSignal("");
  let list!: HTMLUListElement;
  let input!: HTMLInputElement;
  let focusAfter: string | null = null;
  createEffect(() => {
    const f = folder();
    if (!f) return;
    setLastDir(f.dir);
    const wanted = focusAfter;
    if (wanted === null) return;
    focusAfter = null;
    queueMicrotask(() => {
      const items = [...list.querySelectorAll<HTMLElement>("[data-nav]")];
      focusUnlessTouch(items.find((item) => item.dataset.name === wanted) ?? items[0] ?? input);
    });
  });
  onMount(() => {
    if (props.focus) focusUnlessTouch(input);
  });
  const isPath = () => /^(\/|~|\.\/|\.\.\/)/.test(typed().trim());
  const shown = () => {
    const entries = folder()?.entries ?? [];
    const query = typed().trim().toLowerCase();
    return isPath() || !query ? entries : entries.filter((e) => e.name.toLowerCase().includes(query));
  };
  const browse = (to: string, focus: string | null = null) => {
    focusAfter = focus;
    setTyped("");
    setError("");
    setDir(to);
  };
  const up = (focus: boolean) => {
    const f = folder();
    if (f?.parent) browse(f.parent, focus ? (f.dir.split("/").pop() ?? "") : null);
  };
  const open = (name: string, isDir: boolean) => {
    const path = `${folder()?.dir ?? ""}/${name}`;
    if (isDir) browse(path, "");
    else props.onPick(path, []);
  };
  const submit = () => {
    if (!isPath()) {
      const first = shown()[0];
      if (first) open(first.name, first.dir);
      return;
    }
    const value = typed().trim();
    if (value.endsWith("/")) return browse(value);
    const parsed = typedRun(value);
    if (typeof parsed === "string") return setError(parsed);
    props.onPick(parsed.script, parsed.extra);
  };
  return (
    <div class="picker">
      <div class="toolbar">
        <button type="button" class="icon-button" aria-label="Up a folder" data-tip="Up a folder" onClick={() => up(false)} disabled={!folder()?.parent}>
          <Icon name="up" />
        </button>
        <span class="muted ellipsis small" aria-live="polite">
          {hide(folder()?.shown ?? "…")}
        </span>
      </div>
      <label class="field">
        <span class="visually-hidden">Filter, or type a path and args</span>
        <input
          ref={input}
          type="text"
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          autocomplete="off"
          placeholder="Filter, or type a path like ~/scripts/farm.ts key=value"
          value={typed()}
          aria-invalid={error() ? "true" : undefined}
          aria-describedby={error() ? "files-error" : undefined}
          onInput={(e) => {
            setTyped(e.currentTarget.value);
            setError("");
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              list.querySelector<HTMLElement>("[data-nav]")?.focus();
            }
          }}
        />
      </label>
      <Show when={error() || listing.error}>
        <p id="files-error" class="error" role="alert">
          {error() || message(listing.error)}
        </p>
      </Show>
      <ul
        class="entries"
        ref={list}
        aria-label="Scripts and folders"
        onKeyDown={(e) => {
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            up(true);
          } else arrows(e, list);
        }}
      >
        <For each={shown()} fallback={<li class="muted empty">No scripts here</li>}>
          {(entry) => (
            <li>
              <button type="button" data-nav data-name={entry.name} class="entry" onClick={() => open(entry.name, entry.dir)}>
                <Icon name={entry.dir ? "folder" : "file"} />
                <span>{entry.name}</span>
                <Show when={entry.dir}>
                  <span class="visually-hidden"> folder</span>
                </Show>
              </button>
              <Show when={props.options && entry.name.endsWith(".cs")}>
                <button
                  type="button"
                  class="icon-button"
                  aria-label={`Options for ${entry.name}`} data-tip={`Options for ${entry.name}`}
                  onClick={() => setDialog({ kind: "skua", script: `${folder()?.dir ?? ""}/${entry.name}` })}
                >
                  <Icon name="options" />
                </button>
              </Show>
            </li>
          )}
        </For>
      </ul>
    </div>
  );
}

function Skua(props: Pick) {
  const client = useQueryClient();
  const [query, setQuery] = createSignal("");
  const [debounced] = createDebouncedValue(query, { wait: 150 });
  const found = createQuery(() => ({
    queryKey: ["skua", debounced()],
    queryFn: () => api.skua(debounced()),
    placeholderData: keepPreviousData,
  }));
  const library = () => found.data;
  const update = createMutation(() => ({
    mutationFn: api.updateSkua,
    onSuccess: (done) => {
      say(done.message);
      void client.invalidateQueries({ queryKey: ["skua"] });
    },
    onError: (e: Error) => fail(e.message),
  }));
  const updating = () => update.isPending;
  let scroller!: HTMLDivElement;
  let input!: HTMLInputElement;
  onMount(() => {
    if (props.focus) focusUnlessTouch(input);
  });
  return (
    <div class="picker">
      <div class="toolbar">
        <label class="field grow">
          <span class="visually-hidden">Search the Skua scripts</span>
          <input
            ref={input}
            type="search"
            placeholder="Search names, or #tag"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                scroller.querySelector<HTMLElement>("[data-nav]")?.focus();
              }
            }}
          />
        </label>
        <button type="button" disabled={updating()} onClick={() => update.mutate()}>
          {updating() ? "Updating…" : "Update"}
        </button>
        <Show when={(() => { const l = library(); return l?.state === "ready" ? l : undefined; })()}>
          {(l) => <span class="muted small">{l().commit}</span>}
        </Show>
      </div>
      <Switch>
        <Match when={found.error}>
          <p class="error">{message(found.error)}</p>
        </Match>
        <Match when={library()?.state === "missing"}>
          <p class="muted">The Skua scripts aren't downloaded yet. Update downloads them.</p>
        </Match>
        <Match when={(() => { const l = library(); return l?.state === "failed" ? l : undefined; })()}>
          {(l) => <p class="error">{l().error}</p>}
        </Match>
        <Match when={(() => { const l = library(); return l?.state === "ready" ? l : undefined; })()}>
          {(l) => (
            <VirtualList
              label="Skua scripts"
              items={l().entries}
              ref={(el) => (scroller = el)}
              empty="Nothing matches"
              row={(entry) => (
                <>
                  <button type="button" data-nav class="entry stack" onClick={() => props.onPick(entry.path, [])}>
                    <span>{entry.name}</span>
                    <Show when={entry.description}>
                      <span class="muted wrap">{entry.description}</span>
                    </Show>
                    <span class="muted small path">
                      {entry.path.startsWith(`${l().folder}/`) ? entry.path.slice(l().folder.length + 1) : entry.path}
                      {entry.tags.map((t) => `  #${t}`).join("")}
                    </span>
                  </button>
                  <Show when={props.options}>
                    <button
                      type="button"
                      class="icon-button"
                      aria-label={`Options for ${entry.name}`} data-tip={`Options for ${entry.name}`}
                      onClick={() => setDialog({ kind: "skua", script: entry.path })}
                    >
                      <Icon name="options" />
                    </button>
                  </Show>
                </>
              )}
            />
          )}
        </Match>
      </Switch>
    </div>
  );
}

const normal = (path: string) => {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "..") parts.pop();
    else if (part !== "." && part !== "") parts.push(part);
  }
  return `/${parts.join("/")}`;
};

function RecentRuns(props: Pick) {
  const recentRuns = createQuery(() => ({ queryKey: ["recent"], queryFn: api.recent, staleTime: 30_000 }));
  const hidden = () => new Set(recentRuns.data?.hidden ?? []);
  const running = createMemo(
    () => [...new Set(accounts.flatMap((a) => (a.script ? [normal(a.script)] : [])))].filter((p) => !hidden().has(p)),
    undefined,
    { equals: (a, b) => a.length === b.length && a.every((x, i) => x === b[i]) },
  );
  const client = useQueryClient();
  const forget = createMutation(() => ({
    mutationFn: api.forgetRecent,
    onSuccess: (left) => client.setQueryData(["recent"], left),
    onError: (e: Error) => fail(e.message),
  }));
  const runningOn = (path: string) => accounts.filter((a) => a.script && normal(a.script) === path).map((a) => a.name);
  const past = () => {
    const now = new Set(running());
    return (recentRuns.data?.runs ?? []).filter((run) => !now.has(run.path));
  };
  let list!: HTMLUListElement;
  onMount(() => {
    if (props.focus) queueMicrotask(() => focusUnlessTouch(list.querySelector<HTMLElement>("[data-nav]")));
  });
  const remove = (path: string, button: HTMLElement) => {
    const at = [...list.querySelectorAll("li")].findIndex((row) => row.contains(button));
    forget.mutate(path, {
      onSuccess: () =>
        queueMicrotask(() => {
          const left = [...list.querySelectorAll<HTMLElement>("[data-nav]")];
          (left[Math.min(at, left.length - 1)] ?? list).focus();
        }),
    });
  };
  const Remove = (p: { path: string }) => (
    <button
      type="button"
      class="icon-button"
      aria-label={`Remove ${shownName(p.path)} from recent`}
      data-tip="Remove from recent"
      onClick={(e) => remove(p.path, e.currentTarget)}
    >
      <Icon name="close" />
    </button>
  );
  return (
    <ul class="entries" ref={list} tabindex="-1" aria-label="Recent scripts" onKeyDown={(e) => arrows(e, list)}>
      <For each={running()}>
        {(path) => (
          <li>
            <button type="button" data-nav class="entry stack" onClick={() => props.onPick(path, [])}>
              <span>{shownName(path)}</span>
              <span class="muted ellipsis">running on {runningOn(path).map(hide).join(", ")}</span>
              <span class="muted small path">{hide(path)}</span>
            </button>
            <Remove path={path} />
          </li>
        )}
      </For>
      <For each={past()}>
        {(run) => (
          <li>
            <button type="button" data-nav class="entry stack" onClick={() => props.onPick(run.path, run.args)}>
              <span>{shownName(run.path)}</span>
              <span class="muted ellipsis">{run.args.map(([k, v]) => `${k}=${v}`).join(" ")}</span>
              <span class="muted small path">{hide(run.shown)}</span>
            </button>
            <Remove path={run.path} />
          </li>
        )}
      </For>
      <Show when={recentRuns.data && past().length === 0 && running().length === 0}>
        <li class="muted empty">Nothing here yet. Files and Skua list every script.</li>
      </Show>
    </ul>
  );
}

function argHint(arg: DeclaredArg) {
  const kinds = {
    string: "text",
    number: "a number",
    boolean: "true or false",
    list: "a JSON list",
    map: "a JSON object",
  } as const;
  const parts: string[] = [kinds[arg.kind]];
  if (arg.optional) parts.push(arg.default !== null ? `optional, defaults to ${arg.default}` : "optional");
  return parts.join(", ");
}

function RunArgs(props: { script: string; extra: [string, string][] }) {
  const on = acting();
  const asked = createQuery(() => ({ queryKey: ["args", props.script, on], queryFn: () => api.args(props.script, on), staleTime: 0 }));
  const declared = () => asked.data;
  const [shared, setShared] = createSignal(true);
  const [answers, setAnswers] = createSignal<Record<string, string>>({});
  const [problems, setProblems] = createSignal<Problem[]>([]);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [extra, setExtra] = createSignal(props.extra.map(([k, v]) => `${k}=${v}`).join(" "));
  let form!: HTMLFormElement;
  const key = (account: number | null, arg: string) => `${account ?? "all"}:${arg}`;
  createEffect(() => {
    const d = declared();
    if (!d) return;
    const typed = new Map(props.extra);
    const filled: Record<string, string> = {};
    for (const arg of d.args) {
      const given = typed.get(arg.name);
      typed.delete(arg.name);
      filled[key(null, arg.name)] = given ?? arg.shared ?? arg.each[0] ?? "";
      on.forEach((account, i) => {
        filled[key(account, arg.name)] = given ?? arg.each[i] ?? arg.shared ?? "";
      });
    }
    setAnswers(filled);
    setExtra([...typed].map(([k, v]) => `${k}=${v}`).join(" "));
    queueMicrotask(() => focusUnlessTouch(form.querySelector<HTMLElement>("input")));
  });
  const perAccount = () => on.length > 1 && !shared();
  const problem = (account: number | null, arg: string) =>
    problems().find((p) => p.arg === arg && (account === null || p.account === account));
  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    const d = declared();
    if (!d) return;
    const typed = typedRun(`x ${extra()}`);
    if (typeof typed === "string") return setError(typed);
    const sent = new Map(typed.extra);
    for (const [name] of perAccount() ? [] : props.extra) {
      if (!d.args.some((arg) => arg.name === name)) continue;
      const value = straight(answers()[key(null, name)] ?? "");
      const reread = typedRun(`x ${name}=${value}`);
      const keeps = typeof reread !== "string" && reread.extra.length === 1 && reread.extra[0]?.[1] === value;
      if (value && keeps) sent.set(name, value);
      else sent.delete(name);
    }
    setError("");
    setBusy(true);
    try {
      const outcome = await api.run({
        script: props.script,
        extra: [...sent],
        shared: !perAccount(),
        runs: targetsOf(on).map(({ account, name }) => ({
          account,
          name,
          args: d.args.map((arg): [string, string] => [
            arg.name,
            straight(answers()[key(perAccount() ? account : null, arg.name)] ?? ""),
          ]),
        })),
      });
      if (Array.isArray(outcome)) {
        close();
        report(outcome, `running ${d.shown}`);
      } else {
        setProblems(outcome.problems);
        queueMicrotask(() => form.querySelector<HTMLElement>("[aria-invalid=true]")?.focus());
      }
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  };
  const Field = (p: { arg: DeclaredArg; account: number | null }) => {
    const id = `arg-${p.account ?? "all"}-${p.arg.name}`;
    const bad = () => problem(p.account, p.arg.name);
    return (
      <div class="field">
        <label for={id}>
          {p.arg.name} <span class="muted">{argHint(p.arg)}</span>
        </label>
        <Show
          when={p.arg.kind === "boolean"}
          fallback={
            <input
              id={id}
              type="text"
              inputmode={p.arg.kind === "number" ? "decimal" : undefined}
              spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
              autocomplete="off"
              value={answers()[key(p.account, p.arg.name)] ?? ""}
              aria-invalid={bad() ? "true" : undefined}
              aria-describedby={bad() ? `${id}-error` : undefined}
              onInput={(e) => setAnswers({ ...answers(), [key(p.account, p.arg.name)]: e.currentTarget.value })}
            />
          }
        >
          <select
            id={id}
            value={answers()[key(p.account, p.arg.name)] ?? ""}
            aria-invalid={bad() ? "true" : undefined}
            aria-describedby={bad() ? `${id}-error` : undefined}
            onChange={(e) => setAnswers({ ...answers(), [key(p.account, p.arg.name)]: e.currentTarget.value })}
          >
            <option value="" disabled={!p.arg.optional}>
              {p.arg.optional ? "default" : "choose"}
            </option>
            <option value="true">true</option>
            <option value="false">false</option>
          </select>
        </Show>
        <Show when={bad()}>
          {(b) => (
            <p id={`${id}-error`} class="error">
              {b().error}
            </p>
          )}
        </Show>
      </div>
    );
  };
  return (
    <Modal
      title={`Run ${hide(declared()?.shown ?? shownName(props.script))}`}
      onClose={close}
      lightDismiss={false}
      footer={
        <>
          <Show when={error()}>
            <p class="error" role="alert">
              {error()}
            </p>
          </Show>
          <button type="button" onClick={close}>
            Cancel
          </button>
          <button type="submit" form="run-form" class="primary" disabled={busy() || !declared()}>
            {busy() ? "Starting…" : on.length > 1 ? `Run on ${on.length} accounts` : "Run"}
          </button>
        </>
      }
    >
      <form id="run-form" ref={form} onSubmit={submit} class="form">
        <Show when={asked.error}>
          <p class="error">{message(asked.error)}</p>
        </Show>
        <Show when={on.length > 1}>
          <label class="check">
            <input type="checkbox" checked={shared()} onChange={(e) => setShared(e.currentTarget.checked)} />
            Same answers for every account
          </label>
        </Show>
        <Show
          when={perAccount()}
          fallback={<For each={declared()?.args ?? []}>{(arg) => <Field arg={arg} account={null} />}</For>}
        >
          <For each={on}>
            {(account) => (
              <fieldset>
                <legend>{nameOf(account)}</legend>
                <For each={declared()?.args ?? []}>{(arg) => <Field arg={arg} account={account} />}</For>
              </fieldset>
            )}
          </For>
        </Show>
        <div class="field">
          <label for="run-extra">
            Other args <span class="muted">key=value, separated by spaces</span>
          </label>
          <input
            id="run-extra"
            type="text"
            spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
            autocomplete="off"
            value={extra()}
            onInput={(e) => setExtra(e.currentTarget.value)}
          />
        </div>
      </form>
    </Modal>
  );
}

function Servers() {
  const on = acting();
  const listed = createQuery(() => ({ queryKey: ["servers", on], queryFn: () => api.servers(on), staleTime: 0 }));
  const servers = () => listed.data;
  const currentServer = () => accounts[on[0] ?? 0]?.server ?? "";
  let list!: HTMLUListElement;
  createEffect(() => {
    if (servers()) queueMicrotask(() => focusUnlessTouch(list?.querySelector<HTMLElement>("[aria-current=true], [data-nav]")));
  });
  return (
    <Modal title={on.length === 1 ? `Switch ${nameOf(on[0] ?? 0)} to another server` : `Switch ${on.length} accounts`} onClose={close}>
      <Switch fallback={<p class="muted" aria-live="polite">Loading the servers…</p>}>
        <Match when={listed.error}>
          <p class="error">{message(listed.error)}</p>
        </Match>
        <Match when={servers()}>
          {(list_) => (
            <ul class="entries" ref={list} aria-label="Servers" onKeyDown={(e) => arrows(e, list)}>
              <For each={list_()}>
                {(s) => (
                  <li>
                    <button
                      type="button"
                      data-nav
                      class="entry server"
                      aria-current={s.name.toLowerCase() === currentServer().toLowerCase() ? "true" : undefined}
                      aria-disabled={s.refusal ? "true" : undefined}
                      onClick={() => {
                        if (s.refusal) return fail(`${s.name} ${s.refusal}`);
                        close();
                        void act({ switch: s.name }, `switching to ${s.name}`, on);
                      }}
                    >
                      <span class="ellipsis">{s.name}</span>
                      <span class="muted num">
                        {s.players}/{s.capacity}
                      </span>
                      <span class="muted note">
                        {[
                          s.name.toLowerCase() === currentServer().toLowerCase() ? "current" : "",
                          s.refusal ? s.refusal.replace(/^is /, "") : s.members ? "members" : "",
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          )}
        </Match>
      </Switch>
    </Modal>
  );
}

function SkuaOptions(props: { script: string }) {
  const account = acting()[0] ?? selected() ?? 0;
  const client = useQueryClient();
  const key = ["skuaOptions", account, props.script];
  const name = accounts[account]?.name ?? "";
  const options = createQuery(() => ({ queryKey: key, queryFn: () => api.skuaOptions(account, name, props.script), staleTime: 0 }));
  const loaded = () => options.data;
  const [tab, setTab] = optionsTab;
  const [changes, setChanges] = createSignal<Record<string, string>>({});
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const pane = (): SkuaPane | undefined => loaded()?.[tab()];
  const save = async (e: SubmitEvent) => {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const saved = await api.saveSkuaOptions({
        account,
        name,
        script: props.script,
        tab: tab(),
        changes: Object.entries(changes()),
      });
      const now = loaded();
      if (now) client.setQueryData(key, { ...now, [tab()]: saved });
      setChanges({});
      say("saved the options");
    } catch (err) {
      setError(message(err));
    } finally {
      setBusy(false);
    }
  };
  const keyOf = (o: SkuaOption) => (tab() === "script" ? `${o.group}:${o.name}` : o.name);
  const value = (o: SkuaOption) => changes()[keyOf(o)] ?? o.value ?? "";
  const effective = (o: SkuaOption) => changes()[keyOf(o)] ?? o.value ?? o.default ?? "";
  const set = (o: SkuaOption, v: string) => setChanges({ ...changes(), [keyOf(o)]: v });
  return (
    <Modal
      title={`Options for ${shownName(props.script)} · ${nameOf(account)}`}
      onClose={close}
      lightDismiss={false}
      wide
      footer={
        <>
          <Show when={error()}>
            <p class="error" role="alert">
              {error()}
            </p>
          </Show>
          <button type="button" onClick={close}>
            Close
          </button>
          <button type="submit" form="skua-form" class="primary" disabled={busy() || Object.keys(changes()).length === 0}>
            {busy() ? "Saving…" : "Save"}
          </button>
        </>
      }
    >
      <div class="tabs" role="tablist" aria-label="Options">
        <For each={["corebots", "script"] as const}>
          {(t) => (
            <button
              type="button"
              role="tab"
              aria-selected={tab() === t}
              tabindex={tab() === t ? 0 : -1}
              onClick={() => {
                setTab(t);
                setChanges({});
              }}
              onKeyDown={(e) => {
                if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
                  e.preventDefault();
                  const next = t === "corebots" ? "script" : "corebots";
                  setTab(next);
                  setChanges({});
                  e.currentTarget.parentElement?.querySelector<HTMLElement>(`[aria-selected="false"]`)?.focus();
                }
              }}
            >
              {t === "corebots" ? "CoreBots" : "Script"}
            </button>
          )}
        </For>
      </div>
      <Switch fallback={<p class="muted" aria-live="polite">Loading the options…</p>}>
        <Match when={options.error}>
          <p class="error">{message(options.error)}</p>
        </Match>
        <Match when={(() => { const p = pane(); return p && "error" in p ? p : undefined; })()}>
          {(p) => <p class="error">{p().error}</p>}
        </Match>
        <Match when={(() => { const p = pane(); return p && "options" in p ? p : undefined; })()}>
          {(p) => (
            <form id="skua-form" class="form options" onSubmit={save}>
              <Show when={p().options.length === 0}>
                <p class="muted">This script has no options.</p>
              </Show>
              <For each={p().options}>
                {(o) => {
                  const id = `opt-${keyOf(o)}`;
                  return (
                    <div class="field">
                      <Switch>
                        <Match when={o.kind === "bool"}>
                          <label class="check">
                            <input
                              id={id}
                              type="checkbox"
                              checked={effective(o).toLowerCase() === "true"}
                              onChange={(e) => set(o, e.currentTarget.checked ? "True" : "False")}
                            />
                            {o.label}
                          </label>
                        </Match>
                        <Match when={o.kind === "choice"}>
                          <label for={id}>{o.label}</label>
                          <select id={id} value={effective(o)} onChange={(e) => set(o, e.currentTarget.value)}>
                            <For each={o.choices}>{(c) => <option value={c}>{c}</option>}</For>
                          </select>
                        </Match>
                        <Match when={o.kind === "number" || o.kind === "text"}>
                          <label for={id}>{o.label}</label>
                          <input
                            id={id}
                            type="text"
                            inputmode={o.kind === "number" ? "decimal" : undefined}
                            value={value(o)}
                            placeholder={o.default ?? undefined}
                            onInput={(e) => set(o, e.currentTarget.value)}
                          />
                        </Match>
                      </Switch>
                      <Show when={o.description}>
                        <p class="muted small">{o.description}</p>
                      </Show>
                    </div>
                  );
                }}
              </For>
            </form>
          )}
        </Match>
      </Switch>
    </Modal>
  );
}

function End() {
  const [busy, setBusy] = createSignal(false);
  return (
    <Modal
      title="End the session?"
      onClose={close}
      footer={
        <>
          <button type="button" onClick={close}>
            Cancel
          </button>
          <button
            type="button"
            class="primary danger"
            disabled={busy()}
            onClick={async () => {
              setBusy(true);
              try {
                await api.endSession(startedMs());
                say("logging every account out");
                close();
              } catch (e) {
                fail(message(e));
                setBusy(false);
              }
            }}
          >
            {busy() ? "Logging out…" : "Log everyone out"}
          </button>
        </>
      }
    >
      <p>Every account logs out and the session ends. aqx web keeps running, so you can log back in from here.</p>
    </Modal>
  );
}

function Restart() {
  const restart = createMutation(() => ({
    mutationFn: api.restartSession,
    onSuccess: ({ restarted }) => {
      say(restarted ? "restarted the session with this build" : "ended the old session");
      close();
    },
    onError: (e: Error) => fail(e.message),
  }));
  return (
    <Modal
      title="Restart the session?"
      onClose={close}
      footer={
        <>
          <button type="button" onClick={close}>
            Cancel
          </button>
          <button type="button" class="primary danger" disabled={restart.isPending} onClick={() => restart.mutate(startedMs())}>
            {restart.isPending ? "Restarting…" : "Restart"}
          </button>
        </>
      }
    >
      <p>Every online account logs out, then logs back in and starts its script again. Changes saved to aqx.toml apply.</p>
    </Modal>
  );
}

function Confirm(props: { title: string; body: string; label: string; danger: boolean; run: () => void }) {
  let cancel!: HTMLButtonElement;
  onMount(() => requestAnimationFrame(() => cancel.focus()));
  return (
    <Modal
      title={props.title}
      onClose={close}
      footer={
        <>
          <button type="button" ref={cancel} onClick={close}>
            Cancel
          </button>
          <button
            type="button"
            class="primary"
            classList={{ danger: props.danger }}
            onClick={() => {
              close();
              props.run();
            }}
          >
            {props.label}
          </button>
        </>
      }
    >
      <p>{props.body}</p>
    </Modal>
  );
}
