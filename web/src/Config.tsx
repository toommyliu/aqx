import { createDebouncedValue } from "@tanstack/solid-pacer";
import { createMutation, createQuery, useQueryClient } from "@tanstack/solid-query";
import { For, Index, Match, Show, Switch, createEffect, createMemo, createSignal, createUniqueId, on, onCleanup, onMount } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { type ConfigStart, persisted, session, setDialog } from "./store";
import { ScriptPicker } from "./Dialogs";
import { Icon, Modal, anchor, arrows, clock } from "./ui";
import {
  type ConfigFile,
  type ConfigSchema,
  type DeclaredArg,
  type EditedAccount,
  type Fields,
  type JsonValue,
  type SchemaProperty,
  api,
} from "./wire";

const saved = () =>
  `Saved at ${clock(Date.now()).slice(0, 5)}.${session()?.kind === "running" ? " Restart the session to apply it to online accounts." : ""}`;

const configTab = persisted<"accounts" | "defaults" | "toml">("aqx-config-tab", ["accounts", "defaults", "toml"], "accounts");

const ACCOUNT_KEYS = ["user", "password", "name", "script", "args"];
const IDENTITY = new Set(ACCOUNT_KEYS);

const ORDER = [
  "server",
  "auto_start",
  "auto_relogin",
  "relogin_delay",
  "relogin_attempts",
  "restart_on_error",
  "stall_after",
  "on_stall",
  "room",
  "client_version",
];

const LABELS: Record<string, string> = {
  server: "Server",
  auto_start: "Log in at session start",
  auto_relogin: "Relogin after a drop or kick",
  relogin_delay: "First relogin wait",
  relogin_attempts: "Relogin tries",
  restart_on_error: "Restart the script on error",
  stall_after: "Stalled after",
  on_stall: "When stalled",
  room: "Room",
  client_version: "Client version",
};

type Kind = "server" | "bool" | "choice" | "duration" | "int" | "text";
type Setting = { key: string; label: string; help: string; kind: Kind; pattern: string | undefined };

type ArgKind = "text" | "number" | "bool";
type Arg = { key: string; value: string; kind: ArgKind };
type AccountDraft = {
  id: string;
  from: number | null;
  user: string;
  name: string;
  script: string;
  password: string;
  hasPassword: boolean;
  args: Arg[];
  overrides: Record<string, string>;
};
type Draft = { defaults: Record<string, string>; accounts: AccountDraft[] };
type Problem = { id: string; account: string | null; message: string };

function kindOf(key: string, p: SchemaProperty): Kind {
  if (key === "server") return "server";
  if (p.anyOf?.some((a) => a.$ref?.endsWith("/OnStall"))) return "choice";
  if (p.pattern) return "duration";
  const types = Array.isArray(p.type) ? p.type : [p.type];
  if (types.includes("boolean")) return "bool";
  if (types.includes("integer")) return "int";
  return "text";
}

const rank = (key: string) => (ORDER.includes(key) ? ORDER.indexOf(key) : ORDER.length);

function settingsOf(props: Record<string, SchemaProperty>): Setting[] {
  return Object.entries(props)
    .filter(([key]) => !IDENTITY.has(key))
    .sort(([a], [b]) => rank(a) - rank(b))
    .map(([key, p]) => ({ key, label: LABELS[key] ?? key, help: p.description ?? "", kind: kindOf(key, p), pattern: p.pattern }));
}

const text = (v: JsonValue | undefined) => (v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v));

const strings = (fields: Fields, keep: (key: string) => boolean) =>
  Object.fromEntries(Object.entries(fields).filter(([k]) => keep(k)).map(([k, v]) => [k, text(v)]));

function argsOf(v: JsonValue | undefined): Arg[] {
  if (!v || typeof v !== "object" || Array.isArray(v)) return [];
  return Object.entries(v).map(([key, value]) => ({
    key,
    value: String(value),
    kind: typeof value === "boolean" ? "bool" : typeof value === "number" ? "number" : "text",
  }));
}

function draftOf(file: ConfigFile): Draft {
  return {
    defaults: strings(file.form.defaults, () => true),
    accounts: file.form.accounts.map((a, i) => ({
      id: `a${i}`,
      from: i,
      user: text(a.fields.user),
      name: text(a.fields.name),
      script: text(a.fields.script),
      password: "",
      hasPassword: a.hasPassword,
      args: argsOf(a.fields.args),
      overrides: strings(a.fields, (k) => !IDENTITY.has(k)),
    })),
  };
}

const blank = (id: string): AccountDraft => ({
  id,
  from: null,
  user: "",
  name: "",
  script: "",
  password: "",
  hasPassword: false,
  args: [],
  overrides: {},
});

function fieldsOf(values: Record<string, string>, kinds: Map<string, Kind>): Fields {
  const out: Fields = {};
  for (const [key, raw] of Object.entries(values)) {
    const v = raw.trim();
    if (v === "") continue;
    const kind = kinds.get(key);
    out[key] = kind === "bool" ? v === "true" : kind === "int" ? Number(v) : v;
  }
  return out;
}

const argValue = (a: Arg): JsonValue => (a.kind === "bool" ? a.value === "true" : a.kind === "number" ? Number(a.value) : a.value);

function accountOf(a: AccountDraft, kinds: Map<string, Kind>): EditedAccount {
  const fields = fieldsOf({ ...a.overrides, user: a.user, name: a.name, script: a.script }, kinds);
  const named = a.args.filter((r) => r.key.trim());
  if (named.length > 0) fields.args = Object.fromEntries(named.map((r) => [r.key.trim(), argValue(r)]));
  return { from: a.from, fields, password: a.password || null };
}

const isSkua = (script: string) => script.trim().toLowerCase().endsWith(".cs");

function problemsOf(
  draft: Draft,
  defaults: Setting[],
  account: Setting[],
  declared: Record<string, DeclaredArg[]>,
): Problem[] {
  const out: Problem[] = [];
  const check = (id: string, owner: string | null, raw: string, s: Setting, required: boolean) => {
    const v = raw.trim();
    if (!v) {
      if (required) out.push({ id, account: owner, message: "Pick a value or remove this override" });
    } else if (s.kind === "int" && !/^\d+$/.test(v)) {
      out.push({ id, account: owner, message: "Use a whole number" });
    } else if (s.kind === "duration" && s.pattern && !new RegExp(s.pattern).test(v)) {
      out.push({ id, account: owner, message: "Use a length like 5s, 90s, 5m or 1h" });
    }
  };
  for (const s of defaults) check(`defaults-${s.key}`, null, draft.defaults[s.key] ?? "", s, false);
  for (const a of draft.accounts) {
    const at = `acct-${a.id}`;
    if (!a.user.trim()) out.push({ id: `${at}-user`, account: a.id, message: "Every account needs a username" });
    if (!a.password && !a.hasPassword) out.push({ id: `${at}-password`, account: a.id, message: "A new account needs a password" });
    const known = isSkua(a.script) ? undefined : declared[a.script.trim()];
    a.args.forEach((r, i) => {
      const key = r.key.trim();
      const arg = known?.find((d) => d.name === key);
      if (!key && r.value.trim()) out.push({ id: `${at}-arg${i}-key`, account: a.id, message: "Name this arg" });
      else if (known && key && !arg)
        out.push({
          id: `${at}-arg${i}-key`,
          account: a.id,
          message: known.length > 0 ? `The script has no arg named ${key}` : "The script takes no args",
        });
      if (r.kind === "number" && !(r.value.trim() && Number.isFinite(Number(r.value))))
        out.push({ id: arg ? `${at}-argd-${key}` : `${at}-arg${i}-value`, account: a.id, message: "Use a number" });
    });
    for (const d of known ?? [])
      if (!d.optional && !a.args.some((r) => r.key.trim() === d.name && r.value.trim()))
        out.push({ id: `${at}-argd-${d.name}`, account: a.id, message: "The script needs this" });
    for (const s of account) {
      const raw = a.overrides[s.key];
      if (s.key === "server" && !draft.defaults.server?.trim()) {
        if (!raw?.trim()) out.push({ id: `${at}-server`, account: a.id, message: "Pick a server here or in Defaults" });
      } else if (raw !== undefined) check(`${at}-${s.key}`, a.id, raw, s, true);
    }
  }
  return out;
}

const canonical = (draft: Draft) =>
  JSON.stringify(draft, (_, v: unknown) =>
    v && typeof v === "object" && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => a.localeCompare(b))) : v,
  );

const shown = (v: string) => (v === "true" ? "yes" : v === "false" ? "no" : v);

export function Config(props: { start: ConfigStart }) {
  const client = useQueryClient();
  const file = createQuery(() => ({ queryKey: ["config"], queryFn: api.config, staleTime: 0 }));
  const [tab, setTab] = configTab;
  if (props.start.kind !== "list") setTab("accounts");
  const close = () => setDialog(null);
  const [formDirty, setFormDirty] = createSignal(false);
  const [tomlDirty, setTomlDirty] = createSignal(false);
  const [confirming, setConfirming] = createSignal(false);
  let keep: HTMLButtonElement | undefined;
  const canClose = () => {
    if (!formDirty() && !tomlDirty()) return true;
    setConfirming(true);
    queueMicrotask(() => keep?.focus());
    return false;
  };
  const requestClose = () => {
    if (canClose()) close();
  };
  const saved = (next: ConfigFile) => {
    client.setQueryData(["config"], next);
  };
  const tabs = [
    { id: "accounts", label: "Accounts" },
    { id: "defaults", label: "Defaults" },
    { id: "toml", label: "Raw" },
  ] as const;
  return (
    <Modal title="aqx.toml" onClose={close} canClose={canClose} lightDismiss={false} wide tall>
      <Show when={confirming()}>
        <div class="confirm" role="alert">
          <span class="grow">You have unsaved changes.</span>
          <button type="button" ref={keep} onClick={() => setConfirming(false)}>
            Keep editing
          </button>
          <button type="button" class="danger" onClick={close}>
            Discard
          </button>
        </div>
      </Show>
      <div class="tabs" role="tablist" aria-label="Config">
        <For each={tabs}>
          {(t) => (
            <button
              type="button"
              role="tab"
              id={`config-${t.id}`}
              aria-selected={tab() === t.id}
              tabindex={tab() === t.id ? 0 : -1}
              onClick={() => setTab(t.id)}
              onKeyDown={(e) => {
                const at = tabs.findIndex((x) => x.id === tab());
                const by = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
                const next = tabs[(at + by + tabs.length) % tabs.length];
                if (!by || !next) return;
                e.preventDefault();
                setTab(next.id);
                document.getElementById(`config-${next.id}`)?.focus();
              }}
            >
              {t.label}
            </button>
          )}
        </For>
      </div>
      <Switch fallback={<p class="muted">Loading aqx.toml…</p>}>
        <Match when={file.error}>
          <p class="error">{file.error?.message}</p>
        </Match>
        <Match when={file.data}>
          {(f) => (
            <>
              <div hidden={tab() === "toml"}>
                <FormEditor
                  file={f()}
                  start={props.start}
                  tab={tab() === "defaults" ? "defaults" : "accounts"}
                  onSaved={saved}
                  onClose={requestClose}
                  onDirty={setFormDirty}
                />
              </div>
              <div hidden={tab() !== "toml"}>
                <Toml file={f()} onSaved={saved} onClose={requestClose} onDirty={setTomlDirty} />
              </div>
            </>
          )}
        </Match>
      </Switch>
    </Modal>
  );
}

function FormEditor(props: {
  file: ConfigFile;
  start: ConfigStart;
  tab: "accounts" | "defaults";
  onSaved: (file: ConfigFile) => void;
  onClose: () => void;
  onDirty: (dirty: boolean) => void;
}) {
  const [, setTab] = configTab;
  const schema = (): ConfigSchema => props.file.schema;
  const defaultSettings = createMemo(() => settingsOf(schema().Settings.properties));
  const accountSettings = createMemo(() => settingsOf(schema().AccountConfig.properties));
  const kinds = createMemo(() => new Map([...defaultSettings(), ...accountSettings()].map((s) => [s.key, s.kind])));
  let version = props.file.version;
  let fresh = 0;
  const opened = draftOf(props.file);
  if (props.start.kind === "new") opened.accounts.push(blank(`new${fresh++}`));
  const start = props.start;
  const initial =
    start.kind === "new"
      ? opened.accounts.at(-1)?.id
      : start.kind === "account"
        ? opened.accounts.find((a) => a.user.toLowerCase() === start.user.toLowerCase())?.id
        : undefined;
  const [current, setCurrent] = createSignal(initial ?? opened.accounts[0]?.id ?? "");
  const [base, setBase] = createSignal(canonical(opened));
  const [draft, setDraft] = createStore<Draft>(structuredClone(opened));
  const [tried, setTried] = createSignal(false);
  const [status, setStatus] = createSignal("");
  const [error, setError] = createSignal("");
  const dirty = createMemo(() => canonical(draft) !== base());
  const [declared, setDeclared] = createStore<Record<string, DeclaredArg[]>>({});
  const problems = createMemo(() => problemsOf(draft, defaultSettings(), accountSettings(), declared));
  const problemAt = createMemo(() => {
    const map = new Map<string, string>();
    if (tried()) for (const p of problems()) if (!map.has(p.id)) map.set(p.id, p.message);
    return map;
  });
  const reset = (file: ConfigFile) => {
    version = file.version;
    setDraft(reconcile(draftOf(file)));
    setBase(canonical(draftOf(file)));
    setTried(false);
  };
  createEffect(() => props.onDirty(dirty()));
  createEffect(
    on(
      () => props.file.version,
      (next) => {
        if (next === version || dirty()) return;
        reset(props.file);
      },
      { defer: true },
    ),
  );
  createEffect(
    on(
      dirty,
      (now) => {
        if (now) setStatus("");
      },
      { defer: true },
    ),
  );
  const save = createMutation(() => ({
    mutationFn: (form: { defaults: Fields; accounts: EditedAccount[] }) => api.saveConfig(version, { form }),
    onSuccess: (next) => {
      const at = draft.accounts.findIndex((a) => a.id === current());
      version = next.version;
      props.onSaved(next);
      reset(next);
      setCurrent(draft.accounts[Math.max(0, at)]?.id ?? "");
      setStatus(saved());
    },
    onError: (e: Error) => setError(e.message),
  }));
  const submit = () => {
    setTried(true);
    const first = problems()[0];
    if (first) {
      if (first.account === null) setTab("defaults");
      else {
        setTab("accounts");
        setCurrent(first.account);
      }
      queueMicrotask(() => document.getElementById(first.id)?.focus());
      return;
    }
    setError("");
    save.mutate({ defaults: fieldsOf(draft.defaults, kinds()), accounts: draft.accounts.map((a) => accountOf(a, kinds())) });
  };
  const client = useQueryClient();
  const reload = async () => {
    await client.refetchQueries({ queryKey: ["config"] });
    reset(props.file);
    setError("");
    setStatus("Reloaded aqx.toml. Make your change again.");
  };
  const editing = () => draft.accounts.find((a) => a.id === current());
  const edit = (id: string, fn: (a: AccountDraft) => void) => setDraft("accounts", (a) => a.id === id, produce(fn));
  const focusUser = () => queueMicrotask(() => document.getElementById(`acct-${current()}-user`)?.focus());
  const add = () => {
    const account = blank(`new${fresh++}`);
    setDraft("accounts", draft.accounts.length, account);
    setCurrent(account.id);
    focusUser();
  };
  onMount(() => {
    if (props.start.kind === "new") focusUser();
  });
  const remove = (id: string) => {
    const at = draft.accounts.findIndex((a) => a.id === id);
    const gone = draft.accounts[at];
    setDraft("accounts", (all) => all.filter((a) => a.id !== id));
    setCurrent(draft.accounts[Math.min(at, draft.accounts.length - 1)]?.id ?? "");
    setStatus(`Removed ${gone?.name || gone?.user || "the new account"}. Save to write it to aqx.toml.`);
  };
  const statusText = () => {
    const n = tried() ? problems().length : 0;
    if (n > 0) return `${n} field${n === 1 ? " needs" : "s need"} fixing before you can save.`;
    return status() || (dirty() ? "Unsaved changes" : "");
  };
  return (
    <form
      class="config-form"
      novalidate
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <Show when={props.tab === "accounts"}>
        <div class="split">
          <div class="split-list">
            <ul class="entries" aria-label="Accounts in aqx.toml">
              <For each={draft.accounts}>
                {(a) => (
                  <li>
                    <button
                      type="button"
                      class="entry"
                      aria-current={current() === a.id ? "true" : undefined}
                      onClick={() => setCurrent(a.id)}
                    >
                      <span class="ellipsis">{a.name || a.user || "New account"}</span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
            <button type="button" class="ghost" onClick={add}>
              <Icon name="add" />
              Add account
            </button>
          </div>
          <Show when={editing()} keyed fallback={<p class="muted">No accounts yet.</p>}>
            {(account) => (
              <AccountEditor
                account={account}
                file={props.file}
                settings={accountSettings()}
                defaults={draft.defaults}
                onStall={schema().OnStall.oneOf}
                problem={(id) => problemAt().get(id)}
                edit={(fn) => edit(account.id, fn)}
                onRemove={() => remove(account.id)}
                onDeclared={(script, args) => setDeclared(script, args)}
              />
            )}
          </Show>
        </div>
      </Show>
      <Show when={props.tab === "defaults"}>
        <div class="pane">
          <p class="muted small">Every account uses these unless it overrides them.</p>
          <div class="settings-list">
            <For each={defaultSettings()}>
              {(s) => (
                <SettingRow
                  setting={s}
                  id={`defaults-${s.key}`}
                  value={draft.defaults[s.key] ?? ""}
                  unset="Unset"
                  required={s.key === "server" && draft.accounts.some((a) => !a.overrides.server?.trim())}
                  placeholder={s.key === "client_version" ? props.file.clientVersion : undefined}
                  onStall={schema().OnStall.oneOf}
                  error={problemAt().get(`defaults-${s.key}`)}
                  onChange={(v) => setDraft("defaults", s.key, v)}
                />
              )}
            </For>
          </div>
        </div>
      </Show>
      <div class="dialog-foot sticky">
        <Show when={error()}>
          <p class="error" role="alert">
            {error()}
          </p>
        </Show>
        <p class="muted small grow ellipsis" role="status" title={statusText()}>
          {statusText()}
        </p>
        <button type="button" onClick={() => props.onClose()}>
          Close
        </button>
        <Show when={error().includes("changed since you opened it")}>
          <button type="button" onClick={() => void reload()}>
            Reload
          </button>
        </Show>
        <button type="submit" class="primary" disabled={save.isPending || !dirty()}>
          {save.isPending ? "Saving…" : "Save"}
        </button>
      </div>
    </form>
  );
}

function AccountEditor(props: {
  account: AccountDraft;
  file: ConfigFile;
  settings: Setting[];
  defaults: Record<string, string>;
  onStall: { const: string; description: string }[];
  problem: (id: string) => string | undefined;
  edit: (fn: (a: AccountDraft) => void) => void;
  onRemove: () => void;
  onDeclared: (script: string, args: DeclaredArg[]) => void;
}) {
  const at = () => `acct-${props.account.id}`;
  const heading = createUniqueId();
  const overridesHeading = createUniqueId();
  const argsLabel = createUniqueId();
  const pinned = (key: string) => key === "server" && !props.defaults.server?.trim();
  const overridden = () => props.settings.filter((s) => props.account.overrides[s.key] !== undefined || pinned(s.key));
  const available = () => props.settings.filter((s) => props.account.overrides[s.key] === undefined && !pinned(s.key));
  const title = () => props.account.name || props.account.user || "New account";
  const override = (key: string) => {
    props.edit((a) => {
      a.overrides[key] = "";
    });
    queueMicrotask(() => document.getElementById(`${at()}-${key}`)?.focus());
  };
  const [choosing, setChoosing] = createSignal(false);
  const home = () => props.file.path.slice(0, props.file.path.lastIndexOf("/"));
  const back = () => {
    setChoosing(false);
    queueMicrotask(() => document.getElementById(`${at()}-script`)?.focus());
  };
  return (
    <Show
      when={!choosing()}
      fallback={
        <section
          class="editor script-chooser"
          aria-labelledby={heading}
          onKeyDown={(e) => {
            if (e.key !== "Escape" || e.defaultPrevented) return;
            e.preventDefault();
            back();
          }}
        >
          <div class="editor-head">
            <button type="button" class="ghost" onClick={back}>
              <Icon name="back" />
              {title()}
            </button>
            <h3 id={heading} class="grow ellipsis">
              Choose a script
            </h3>
          </div>
          <ScriptPicker
            pick={{
              options: false,
              onPick: (path) => {
                props.edit((a) => void (a.script = path.startsWith(`${home()}/`) ? path.slice(home().length + 1) : path));
                back();
              },
            }}
          />
        </section>
      }
    >
      <section class="editor" aria-labelledby={heading}>
        <div class="editor-head">
          <h3 id={heading} class="grow ellipsis">
            {title()}
          </h3>
          <button type="button" class="ghost danger" onClick={() => props.onRemove()}>
            Remove account
          </button>
        </div>
        <div class="identity">
          <label class="label" for={`${at()}-user`}>
            Username
            <Required />
          </label>
          <TextInput
            id={`${at()}-user`}
            required
            value={props.account.user}
            error={props.problem(`${at()}-user`)}
            autocomplete="username"
            onInput={(v) => props.edit((a) => void (a.user = v))}
          />
          <label class="label" for={`${at()}-password`}>
            Password
            <Show when={!props.account.hasPassword}>
              <Required />
            </Show>
          </label>
          <TextInput
            id={`${at()}-password`}
            type="password"
            required={!props.account.hasPassword}
            value={props.account.password}
            placeholder={props.account.hasPassword ? "Leave empty to keep the saved one" : ""}
            error={props.problem(`${at()}-password`)}
            autocomplete="new-password"
            onInput={(v) => props.edit((a) => void (a.password = v))}
          />
          <label class="label" for={`${at()}-name`}>
            Display name
          </label>
          <TextInput
            id={`${at()}-name`}
            value={props.account.name}
            placeholder="Same as the username"
            onInput={(v) => props.edit((a) => void (a.name = v))}
          />
          <label class="label" for={`${at()}-script`}>
            Script
          </label>
          <ScriptField
            id={`${at()}-script`}
            value={props.account.script}
            onChange={(v) => props.edit((a) => void (a.script = v))}
            onChoose={() => {
            setChoosing(true);
            requestAnimationFrame(() => {
              const picker = document.querySelector(".script-chooser");
              if (picker && !picker.contains(document.activeElement))
                picker.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus();
            });
          }}
          />
          <span class="label" id={argsLabel}>
            Script args
          </span>
          <Args
            id={at()}
            labelledBy={argsLabel}
            script={props.account.script}
            args={props.account.args}
            problem={props.problem}
            edit={props.edit}
            onDeclared={props.onDeclared}
          />
        </div>
        <section class="overrides" aria-labelledby={overridesHeading}>
          <div class="overrides-head">
            <div class="grow">
              <h4 id={overridesHeading}>Overrides</h4>
              <p class="muted small">
                {overridden().length === 0
                  ? "None. This account uses Defaults for every setting."
                  : "Settings this account sets itself. The rest come from Defaults."}
              </p>
            </div>
            <Show when={available().length > 0}>
              <OverrideMenu settings={available()} onPick={override} />
            </Show>
          </div>
          <Show when={overridden().length > 0}>
            <div class="settings-list">
              <For each={overridden()}>
                {(s) => (
                  <SettingRow
                    setting={s}
                    id={`${at()}-${s.key}`}
                    value={props.account.overrides[s.key] ?? ""}
                    unset={null}
                    required={pinned(s.key)}
                    inherited={props.defaults[s.key]}
                    onStall={props.onStall}
                    error={props.problem(`${at()}-${s.key}`)}
                    onChange={(v) => props.edit((a) => void (a.overrides[s.key] = v))}
                    onRemove={pinned(s.key) ? undefined : () => props.edit((a) => void delete a.overrides[s.key])}
                  />
                )}
              </For>
            </div>
          </Show>
        </section>
      </section>
    </Show>
  );
}

function SettingRow(props: {
  setting: Setting;
  id: string;
  value: string;
  unset: string | null;
  required?: boolean;
  placeholder?: string;
  inherited?: string | undefined;
  onStall: { const: string; description: string }[];
  error: string | undefined;
  onChange: (v: string) => void;
  onRemove?: () => void;
}) {
  const label = `${props.id}-label`;
  const help = `${props.id}-help`;
  const err = `${props.id}-error`;
  const described = () => [help, props.error ? err : ""].filter(Boolean).join(" ");
  const choices = (): [string, string, string?][] => {
    const unset: [string, string][] = props.unset === null ? [] : [["", props.unset]];
    if (props.setting.kind === "bool")
      return [
        ...unset,
        ["true", "Yes"],
        ["false", "No"],
      ];
    return [...unset, ...props.onStall.map((c): [string, string, string] => [c.const, c.const, c.description])];
  };
  return (
    <div class="setting">
      <div class="setting-text">
        <Show
          when={props.setting.kind === "bool" || props.setting.kind === "choice"}
          fallback={
            <label id={label} for={props.id}>
              {props.setting.label}
              <Show when={props.required}>
                <Required />
              </Show>
            </label>
          }
        >
          <span id={label}>
            {props.setting.label}
            <Show when={props.required}>
              <Required />
            </Show>
          </span>
        </Show>
        <p id={help} class="muted small">
          {props.setting.help}
          <Show when={props.inherited}>{(v) => <> Defaults tab: {shown(v())}.</>}</Show>
        </p>
      </div>
      <div class="setting-control row">
        <Switch>
          <Match when={props.setting.kind === "server"}>
            <ServerSelect
              id={props.id}
              value={props.value}
              unset={props.unset === null ? "Pick a server" : "None"}
              required={!!props.required}
              describedBy={described()}
              invalid={!!props.error}
              onChange={props.onChange}
            />
          </Match>
          <Match when={props.setting.kind === "bool" || props.setting.kind === "choice"}>
            <Segments
              id={props.id}
              value={props.value}
              choices={choices()}
              labelledBy={label}
              describedBy={described()}
              invalid={!!props.error}
              onChange={props.onChange}
            />
          </Match>
          <Match when={true}>
            <input
              id={props.id}
              type="text"
              classList={{ short: props.setting.kind !== "text" || props.setting.key === "client_version" }}
              inputmode={props.setting.kind === "int" ? "numeric" : "text"}
              spellcheck={false}
              autocapitalize="off"
              autocorrect="off"
              autocomplete="off"
              placeholder={props.placeholder}
              value={props.value}
              aria-invalid={props.error ? "true" : undefined}
              aria-describedby={described()}
              onInput={(e) => props.onChange(e.currentTarget.value)}
            />
          </Match>
        </Switch>
        <Show when={props.onRemove}>
          {(remove) => (
            <button
              type="button"
              class="icon-button"
              aria-label={`Stop overriding ${props.setting.label.toLowerCase()}`}
              data-tip="Use the default"
              onClick={() => remove()()}
            >
              <Icon name="close" />
            </button>
          )}
        </Show>
      </div>
      <Show when={props.error}>
        <p id={err} class="error small">
          {props.error}
        </p>
      </Show>
    </div>
  );
}

function Required() {
  return (
    <span class="required-mark" aria-hidden="true">
      *
    </span>
  );
}

const firstSentence = (text: string) => text.split(/(?<=\.)\s/)[0] ?? text;

function OverrideMenu(props: { settings: Setting[]; onPick: (key: string) => void }) {
  let button!: HTMLButtonElement;
  let menu!: HTMLDivElement;
  let release: (() => void) | undefined;
  onCleanup(() => release?.());
  const id = createUniqueId();
  return (
    <>
      <button type="button" class="ghost add" popovertarget={id} aria-haspopup="true" ref={button}>
        <Icon name="add" />
        Override a setting
      </button>
      <div
        id={id}
        class="menu settings-menu"
        popover
        ref={menu}
        on:beforetoggle={(e: ToggleEvent) => {
          release?.();
          release = e.newState === "open" ? anchor(button, menu) : undefined;
        }}
        on:toggle={(e: ToggleEvent) => {
          if (e.newState === "open") requestAnimationFrame(() => menu.querySelector("button")?.focus());
        }}
        onKeyDown={(e) => arrows(e, menu, "button")}
      >
        <For each={props.settings}>
          {(s) => (
            <button
              type="button"
              onClick={() => {
                menu.hidePopover();
                props.onPick(s.key);
              }}
            >
              {s.label}
              <span class="muted small">{firstSentence(s.help)}</span>
            </button>
          )}
        </For>
      </div>
    </>
  );
}

function Segments(props: {
  id: string;
  value: string;
  choices: [string, string, string?][];
  labelledBy: string;
  describedBy?: string;
  invalid: boolean;
  onChange: (v: string) => void;
}) {
  return (
    <div
      class="segments"
      role="radiogroup"
      aria-labelledby={props.labelledBy}
      aria-describedby={props.describedBy}
      aria-invalid={props.invalid ? "true" : undefined}
    >
      <For each={props.choices}>
        {([value, label, title], i) => (
          <label class="segment" title={title}>
            <input
              type="radio"
              id={i() === 0 ? props.id : undefined}
              name={props.id}
              value={value}
              checked={props.value === value}
              onChange={() => props.onChange(value)}
            />
            {label}
          </label>
        )}
      </For>
    </div>
  );
}

function TextInput(props: {
  id: string;
  value: string;
  onInput: (value: string) => void;
  error?: string | undefined;
  required?: boolean;
  type?: "text" | "password";
  placeholder?: string;
  autocomplete?: string;
}) {
  const err = `${props.id}-error`;
  const [revealed, setRevealed] = createSignal(false);
  return (
    <div class="control">
      <div class="reveal">
        <input
          id={props.id}
          type={props.type === "password" && !revealed() ? "password" : "text"}
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          autocomplete={props.autocomplete ?? "off"}
          placeholder={props.placeholder}
          value={props.value}
          aria-required={props.required ? "true" : undefined}
          aria-invalid={props.error ? "true" : undefined}
          aria-describedby={props.error ? err : undefined}
          onInput={(e) => props.onInput(e.currentTarget.value)}
        />
        <Show when={props.type === "password"}>
          <button
            type="button"
            class="icon-button"
            aria-label={revealed() ? "Hide the password" : "Show the password"}
            aria-pressed={revealed()}
            data-tip={revealed() ? "Hide" : "Show"}
            onClick={() => setRevealed(!revealed())}
          >
            <Icon name={revealed() ? "hide" : "show"} />
          </button>
        </Show>
      </div>
      <Show when={props.error}>
        <p id={err} class="error small">
          {props.error}
        </p>
      </Show>
    </div>
  );
}

const hintOf = (d: DeclaredArg) =>
  [
    d.kind === "list" ? "a JSON list" : d.kind === "map" ? "a JSON object" : d.kind === "number" ? "a number" : "",
    d.optional ? (d.default ? `default ${d.default}` : "optional") : "",
  ]
    .filter(Boolean)
    .join(", ");

function Args(props: {
  id: string;
  labelledBy: string;
  script: string;
  args: Arg[];
  problem: (id: string) => string | undefined;
  edit: (fn: (a: AccountDraft) => void) => void;
  onDeclared: (script: string, args: DeclaredArg[]) => void;
}) {
  const [script] = createDebouncedValue(() => props.script.trim(), { wait: 300 });
  const asked = createQuery(() => ({
    queryKey: ["declared", script()],
    queryFn: () => api.args(script(), []),
    enabled: script() !== "" && !isSkua(script()),
    retry: false,
    staleTime: 30_000,
  }));
  const declared = () => (script() && !isSkua(script()) && asked.data ? asked.data.args : null);
  createEffect(() => {
    const known = declared();
    if (known) props.onDeclared(script(), known);
  });
  const names = () => new Set((declared() ?? []).map((d) => d.name));
  const valueOf = (name: string) => props.args.find((r) => r.key.trim() === name)?.value ?? "";
  const answer = (d: DeclaredArg, v: string) =>
    props.edit((a) => {
      const at = a.args.findIndex((r) => r.key.trim() === d.name);
      const row = a.args[at];
      if (v === "") {
        if (row) a.args.splice(at, 1);
        return;
      }
      const kind: ArgKind = d.kind === "number" ? "number" : d.kind === "boolean" ? "bool" : "text";
      if (row) {
        row.value = v;
        row.kind = kind;
      } else a.args.push({ key: d.name, value: v, kind });
    });
  const set = (i: number, fn: (arg: Arg) => void) =>
    props.edit((a) => {
      const arg = a.args[i];
      if (arg) fn(arg);
    });
  return (
    <div class="control args" role="group" aria-labelledby={props.labelledBy}>
      <For each={declared() ?? []}>
        {(d) => {
          const id = `${props.id}-argd-${d.name}`;
          const error = () => props.problem(id);
          return (
            <div class="arg declared">
              <label id={`${id}-label`} for={id}>
                <code>{d.name}</code>
                <Show when={!d.optional}>
                  <Required />
                </Show>
              </label>
              <Show
                when={d.kind === "boolean"}
                fallback={
                  <input
                    id={id}
                    type="text"
                    placeholder={hintOf(d)}
                    inputmode={d.kind === "number" ? "decimal" : "text"}
                    spellcheck={false}
                    autocapitalize="off"
                    autocorrect="off"
                    autocomplete="off"
                    value={valueOf(d.name)}
                    aria-required={d.optional ? undefined : "true"}
                    aria-invalid={error() ? "true" : undefined}
                    onInput={(e) => answer(d, e.currentTarget.value)}
                  />
                }
              >
                <Segments
                  id={id}
                  value={valueOf(d.name)}
                  choices={[
                    ...(d.optional ? [["", d.default ? `Default (${shown(d.default)})` : "Default"] as [string, string]] : []),
                    ["true", "Yes"],
                    ["false", "No"],
                  ]}
                  labelledBy={`${id}-label`}
                  invalid={!!error()}
                  onChange={(v) => answer(d, v)}
                />
              </Show>
              <Show when={error()}>{(message) => <p class="error small">{message()}</p>}</Show>
            </div>
          );
        }}
      </For>
      <Index each={props.args}>
        {(arg, i) => (
          <Show when={!names().has(arg().key.trim())}>
            {(() => {
                const id = `${props.id}-arg${i}`;
                const keyError = () => props.problem(`${id}-key`);
                const valueError = () => props.problem(`${id}-value`);
                return (
                  <div class="arg">
                    <input
                      id={`${id}-key`}
                      type="text"
                      aria-label={`Arg ${i + 1} name`}
                      placeholder="name"
                      spellcheck={false}
                      autocapitalize="off"
                      autocorrect="off"
                      autocomplete="off"
                      value={arg().key}
                      aria-invalid={keyError() ? "true" : undefined}
                      onInput={(e) => {
                        const v = e.currentTarget.value;
                        set(i, (r) => void (r.key = v));
                      }}
                    />
                    <select
                      aria-label={`Arg ${i + 1} type`}
                      value={arg().kind}
                      onChange={(e) => {
                        const kind = e.currentTarget.value as ArgKind;
                        set(i, (r) => {
                          r.kind = kind;
                          if (kind === "bool") r.value = r.value === "true" ? "true" : "false";
                        });
                      }}
                    >
                      <option value="text">text</option>
                      <option value="number">number</option>
                      <option value="bool">yes/no</option>
                    </select>
                    <Show
                      when={arg().kind === "bool"}
                      fallback={
                        <input
                          id={`${id}-value`}
                          type="text"
                          aria-label={`Arg ${i + 1} value`}
                          placeholder="value"
                          inputmode={arg().kind === "number" ? "decimal" : "text"}
                          spellcheck={false}
                          autocapitalize="off"
                          autocorrect="off"
                          autocomplete="off"
                          value={arg().value}
                          aria-invalid={valueError() ? "true" : undefined}
                          onInput={(e) => {
                            const v = e.currentTarget.value;
                            set(i, (r) => void (r.value = v));
                          }}
                        />
                      }
                    >
                      <Segments
                        id={`${id}-value`}
                        value={arg().value}
                        choices={[
                          ["true", "Yes"],
                          ["false", "No"],
                        ]}
                        labelledBy={`${id}-key`}
                        invalid={false}
                        onChange={(v) => set(i, (r) => void (r.value = v))}
                      />
                    </Show>
                    <button
                      type="button"
                      class="icon-button"
                      aria-label={`Remove arg ${i + 1}`}
                      onClick={() => props.edit((a) => void a.args.splice(i, 1))}
                    >
                      <Icon name="close" />
                    </button>
                    <Show when={keyError() ?? valueError()}>{(message) => <p class="error small">{message()}</p>}</Show>
                  </div>
                );
            })()}
          </Show>
        )}
      </Index>
      <Show when={declared()?.length === 0 && props.args.length === 0}>
        <p class="muted small">This script takes no args.</p>
      </Show>
      <Show when={script() && !isSkua(script()) ? asked.error : null}>
        {(e) => <p class="muted small">{e().message}</p>}
      </Show>
      <Show when={declared() === null}>
        <button
          type="button"
          class="ghost add"
          onClick={() => {
            props.edit((a) => void a.args.push({ key: "", value: "", kind: "text" }));
            queueMicrotask(() => document.getElementById(`${props.id}-arg${props.args.length - 1}-key`)?.focus());
          }}
        >
          <Icon name="add" />
          Add arg
        </button>
      </Show>
    </div>
  );
}

function Toml(props: {
  file: ConfigFile;
  onSaved: (file: ConfigFile) => void;
  onClose: () => void;
  onDirty: (dirty: boolean) => void;
}) {
  const [text, setText] = createSignal(props.file.text);
  const [status, setStatus] = createSignal("");
  let base = props.file;
  createEffect(() => props.onDirty(text() !== base.text));
  createEffect(
    on(
      () => props.file,
      (file) => {
        if (text() !== base.text) return;
        setText(file.text);
        base = file;
      },
      { defer: true },
    ),
  );
  onCleanup(() => props.onDirty(false));
  const rawStatus = () => status() || (text() !== base.text ? "Unsaved changes" : props.file.path);
  const save = createMutation(() => ({
    mutationFn: (now: string) => api.saveConfig(base.version, { text: now }),
    onSuccess: (next) => {
      setText(next.text);
      props.onSaved(next);
      setStatus(saved());
    },
  }));
  return (
    <form
      class="config-form"
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate(text());
      }}
    >
      <div class="raw">
        <label for="config-text" class="visually-hidden">
          aqx.toml
        </label>
        <textarea
          id="config-text"
          class="code"
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          autocomplete="off"
          rows={24}
          value={text()}
          aria-invalid={save.error ? "true" : undefined}
          aria-describedby={save.error ? "config-error" : undefined}
          onInput={(e) => {
            setText(e.currentTarget.value);
            setStatus("");
          }}
          onKeyDown={(e) => {
            if (e.key === "s" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              e.currentTarget.form?.requestSubmit();
            }
          }}
        />
        <Reference schema={props.file.schema} />
      </div>
      <div class="dialog-foot sticky">
        <Show when={save.error}>
          <p id="config-error" class="error" role="alert">
            {save.error?.message}
          </p>
        </Show>
        <p class="muted small grow ellipsis" role="status" title={rawStatus()}>
          {rawStatus()}
        </p>
        <button type="button" onClick={() => props.onClose()}>
          Close
        </button>
        <button type="submit" class="primary" disabled={save.isPending || text() === base.text}>
          {save.isPending ? "Saving…" : "Save"}
        </button>
      </div>
    </form>
  );
}

function typeOf(p: SchemaProperty, onStall: { const: string }[]): string {
  if (p.anyOf?.some((a) => a.$ref?.endsWith("/OnStall"))) return onStall.map((c) => `"${c.const}"`).join(" | ");
  if (p.pattern) return "duration";
  const types = Array.isArray(p.type) ? p.type : [p.type];
  if (types.includes("boolean")) return "true | false";
  if (types.includes("integer")) return "integer";
  if (types.includes("object")) return "table";
  return "string";
}

const listed = (words: string[]) =>
  words.length < 2 ? (words[0] ?? "") : `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;

function Reference(props: { schema: ConfigSchema }) {
  const heading = createUniqueId();
  const onStall = () => props.schema.OnStall.oneOf;
  const defaults = () => Object.entries(props.schema.Settings.properties).sort(([a], [b]) => rank(a) - rank(b));
  const account = () =>
    ACCOUNT_KEYS.flatMap((key): [string, SchemaProperty][] => {
      const p = props.schema.AccountConfig.properties[key];
      return p ? [[key, p]] : [];
    });
  const required = () => props.schema.AccountConfig.required ?? [];
  const defaultsOnly = () => Object.keys(props.schema.Settings.properties).filter((k) => !(k in props.schema.AccountConfig.properties));
  const entries = (list: [string, SchemaProperty][], must: string[]) => (
    <dl>
      <For each={list}>
        {([key, p]) => (
          <>
            <dt>
              <code>{key}</code>
              <span class="muted small">{typeOf(p, onStall())}</span>
              <Show when={must.includes(key)}>
                <span class="small required">required</span>
              </Show>
            </dt>
            <dd class="muted small">{p.description}</dd>
          </>
        )}
      </For>
    </dl>
  );
  return (
    <aside class="reference" aria-labelledby={heading}>
      <h3 id={heading} class="visually-hidden">
        Keys aqx.toml takes
      </h3>
      <section>
        <h4>
          <code>[defaults]</code>
        </h4>
        <p class="muted small">Every account uses these unless it sets its own.</p>
        {entries(defaults(), [])}
      </section>
      <section>
        <h4>
          <code>[[account]]</code>
        </h4>
        <p class="muted small">
          One table per account. It takes these keys, plus any <code>[defaults]</code> key
          <Show when={defaultsOnly().length > 0}> except {listed(defaultsOnly())}</Show>.
        </p>
        {entries(account(), required())}
      </section>
    </aside>
  );
}

function ScriptField(props: { id: string; value: string; onChange: (v: string) => void; onChoose: () => void }) {
  return (
    <div class="control row">
      <input
        id={props.id}
        class="grow"
        type="text"
        spellcheck={false}
        autocapitalize="off"
        autocorrect="off"
        autocomplete="off"
        placeholder="No script"
        value={props.value}
        onInput={(e) => props.onChange(e.currentTarget.value)}
      />
      <button type="button" onClick={() => props.onChoose()}>
        Choose…
      </button>
      <Show when={props.value}>
        <button type="button" class="icon-button" aria-label="No script" data-tip="No script" onClick={() => props.onChange("")}>
          <Icon name="close" />
        </button>
      </Show>
    </div>
  );
}

function ServerSelect(props: {
  id: string;
  value: string;
  unset: string;
  required: boolean;
  describedBy: string;
  invalid: boolean;
  onChange: (v: string) => void;
}) {
  const servers = createQuery(() => ({ queryKey: ["servers", "all"], queryFn: api.allServers, staleTime: 60_000 }));
  const match = () => (servers.data ?? []).find((s) => s.name.toLowerCase() === props.value.toLowerCase());
  return (
    <select
      id={props.id}
      aria-required={props.required ? "true" : undefined}
      aria-describedby={props.describedBy}
      aria-invalid={props.invalid ? "true" : undefined}
      onChange={(e) => props.onChange(e.currentTarget.value)}
    >
      <option value="" selected={props.value === ""}>
        {servers.isPending ? "Loading the servers…" : servers.error ? "Couldn't list the servers" : props.unset}
      </option>
      <Show when={props.value && !match()}>
        <option value={props.value} selected>
          {props.value}
        </option>
      </Show>
      <For each={servers.data ?? []}>
        {(s) => (
          <option value={s.name} selected={s === match()}>
            {s.name} · {s.players}/{s.capacity}
            {s.members ? " · members" : ""}
            {s.online ? "" : " · offline"}
          </option>
        )}
      </For>
    </select>
  );
}
