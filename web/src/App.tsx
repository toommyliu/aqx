import { type AppHotkeyId } from "@tanstack/hotkeys";
import {
  createFormatForDisplay,
  createHotkey,
  createHotkeyHint,
  createHotkeys,
  useAppHotkeys,
} from "@tanstack/solid-hotkeys";
import { For, Match, Show, Switch, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { Toaster, useSonner } from "solid-sonner";
import { AccountPanel, LoginMenu } from "./Account";
import { Dialogs, logOut, openPicker, openServers, openScriptOptions } from "./Dialogs";
import { inBrowserTab } from "./hotkeys";
import {
  accounts,
  act,
  connected,
  current,
  dialog,
  chosen,
  ended,
  lastLines,
  lineText,
  here,
  hide,
  isMarked,
  markAll,
  markRange,
  markWhere,
  marked,
  ordered,
  paint,
  redaction,
  say,
  select,
  selected,
  session,
  setDialog,
  setMarked,
  setRedaction,
  setSearch,
  search,
  startFeed,
  toggleMark,
} from "./store";
import { Icon, type IconName, ServerName, Since, Tooltips, anchor, arrows, canHover, clock, number } from "./ui";
import { type Account, isOffline, isOnline, isRunning, reason, stateLabel } from "./wire";

export function pauseOrResume(on: number[]) {
  const paused = on.length > 0 && on.every((i) => accounts[i]?.state === "Paused");
  return paused ? act("resume", "resumed", on) : act("pause", "pausing", on);
}

const JUMPS = ["Mod+1", "Mod+2", "Mod+3", "Mod+4", "Mod+5", "Mod+6", "Mod+7", "Mod+8", "Mod+9"] as const;
const REVEAL_MS = 300;

const interactive = (target: EventTarget | null) =>
  target instanceof HTMLElement && target.closest("button, a, [role=tab], select, summary") !== null;

function Shortcuts(props: { onOpen: (i: number | null) => void }) {
  const keys = useAppHotkeys();
  const bind = (
    id: AppHotkeyId,
    run: (e: KeyboardEvent) => void,
    options: { preventDefault?: boolean; ignoreInputs?: boolean } = {},
  ) =>
    createHotkey(
      () => keys()[id],
      (e) => run(e),
      () => ({ enabled: dialog() === null, ...options }),
    );
  const step = (by: number) => {
    const order = ordered();
    if (order.length === 0) return;
    const at = order.indexOf(selected() ?? -1);
    const to = at === -1 ? (by > 0 ? 0 : order.length - 1) : Math.min(order.length - 1, Math.max(0, at + by));
    select(order[to] ?? null);
  };
  bind("account.login", () => act("login", "logging in", here()));
  bind("account.loginOnly", () => act("login_only", "logging in", here()));
  bind("account.run", () => openPicker(here()));
  bind("account.stop", () => act("stop", "stopping", here()));
  bind("account.restart", () => act("restart", "restarting", here()));
  bind("account.pause", () => pauseOrResume(here()));
  bind("account.options", () => void openScriptOptions(here()));
  bind("account.server", () => openServers(here()));
  bind("account.logout", () => logOut(here()));
  createHotkeys(
    () => JUMPS.slice(0, accounts.length).map((hotkey, i) => ({ hotkey, callback: () => select(i) })),
    () => ({ enabled: !inBrowserTab && dialog() === null }),
  );
  bind("account.next", () => step(1));
  bind("account.previous", () => step(-1));
  let last: string | undefined;
  createEffect(() => {
    const user = current()?.user;
    if (user !== undefined) last = user;
  });
  bind("account.overview", () => {
    if (selected() !== null) return props.onOpen(null);
    const back = accounts.findIndex((a) => a.user === last);
    props.onOpen(back === -1 ? (ordered()[0] ?? null) : back);
  });
  bind(
    "account.mark",
    (e) => {
      const row = e.target instanceof HTMLElement ? e.target.closest<HTMLElement>(".accounts [data-index]") : null;
      if (!row && interactive(e.target)) return;
      const i = row ? Number(row.dataset.index) : selected();
      if (i === null) return;
      e.preventDefault();
      setSelecting(true);
      toggleMark(i);
    },
    { preventDefault: false },
  );
  bind(
    "account.markAll",
    (e) => {
      const inPanel = document.activeElement instanceof Element && document.activeElement.closest(".panel") !== null;
      if (inPanel) return;
      e.preventDefault();
      setSelecting(true);
      markAll();
    },
    { preventDefault: false, ignoreInputs: true },
  );
  bind("search.focus", () => document.getElementById("search")?.focus());
  bind("app.back", () => (search() ? setSearch("") : stopSelecting()));
  bind("app.keys", () => setDialog({ kind: "keys" }));
  return null;
}

export function App() {
  startFeed();
  const [list, setList] = createSignal(true);
  const open = (i: number | null) => {
    select(i);
    setList(false);
    queueMicrotask(() => document.getElementById(i === null ? "dashboard-title" : "account-name")?.focus());
  };
  return (
    <>
      <a class="skip" href="#main">
        Skip to content
      </a>
      <header class="top">
        <button
          type="button"
          class="ghost icon-button sidebar-toggle"
          aria-label={collapsed() ? "Show the account list" : "Hide the account list"}
          aria-pressed={!collapsed()}
          data-tip={collapsed() ? "Show the account list" : "Hide the account list"}
          onClick={() => setCollapsed(!collapsed())}
        >
          <Icon name={collapsed() ? "showList" : "hideList"} />
        </button>
        <div class="title">
          <h1 class="brand">aqx</h1>
          <SessionStatus />
        </div>
        <RedactButton />
        <ThemeButton />
        <button type="button" class="ghost" onClick={() => setDialog({ kind: "config", start: { kind: "list" } })}>
          <Icon name="config" />
          <span class="label">Config</span>
        </button>
        <button type="button" class="ghost keys-button" onClick={() => setDialog({ kind: "keys" })}>
          <Icon name="keyboard" />
          <span class="label">Keys</span>
        </button>
        <SessionMenu />
      </header>
      <div class="shell" classList={{ "show-list": list(), collapsed: collapsed() }}>
        <AccountList onOpen={open} />
        <main id="main" tabindex="-1">
          <button
            type="button"
            class="ghost back"
            onClick={() => {
              setList(true);
              queueMicrotask(() =>
                document.querySelector<HTMLElement>(".accounts [aria-current=true], .accounts button[data-nav]")?.focus(),
              );
            }}
          >
            <Icon name="back" />
            Accounts
          </button>
          <Show when={current()} fallback={<Dashboard onOpen={open} />}>
            {(account) => <AccountPanel account={account()} index={selected() ?? 0} />}
          </Show>
        </main>
      </div>
      <Toasts />
      <Dialogs />
      <Shortcuts onOpen={open} />
      <Tooltips />
    </>
  );
}

function SessionStatus() {
  return (
    <p class="session">
      <Show when={!connected()}>
        <span class="badge warn">reconnecting</span>{" "}
      </Show>
      <Switch>
        <Match when={session()?.kind === "none"}>
          <span class="muted">
            not running
            <Show when={ended()}>{(e) => <> · last session ended at {clock(e().endedMs).slice(0, 5)}</>}</Show>
            {" · "}log an account in to start
          </span>
        </Match>
        <Match when={session()?.kind === "ending"}>
          <span class="muted">ending…</span>
        </Match>
        <Match when={session()?.kind === "incompatible"}>
          <span>started by an incompatible build</span>
        </Match>
        <Match when={(() => { const s = session(); return s?.kind === "running" ? s : undefined; })()}>
          {(s) => (
            <>
              <span class="muted">
                <Since at={s().startedMs} prefix="running for" />
              </span>
              <Show when={s().configChanged}>
                <span
                  class="badge warn"
                  tabindex="0"
                  data-tip="aqx.toml changed after this session started. Restart it from the Session menu to apply changes to accounts already in it."
                >
                  config changed
                </span>
              </Show>
              <Show when={s().otherBuild}>
                <span class="badge warn" tabindex="0" data-tip="The session runs an older aqx build. Restart it from the Session menu to use this one.">
                  older build
                </span>
              </Show>
            </>
          )}
        </Match>
      </Switch>
    </p>
  );
}

function stored(key: string) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    return;
  }
}

const [collapsed, setCollapsedSignal] = createSignal(stored("aqx-sidebar") === "hidden");
const setCollapsed = (hidden: boolean) => {
  setCollapsedSignal(hidden);
  store("aqx-sidebar", hidden ? "hidden" : "shown");
};

type Theme = "system" | "light" | "dark";
const THEMES: Theme[] = ["system", "light", "dark"];

function readTheme(): Theme {
  try {
    const saved = localStorage.getItem("aqx-theme");
    return saved === "light" || saved === "dark" ? saved : "system";
  } catch {
    return "system";
  }
}

export function applyTheme(theme: Theme = readTheme()) {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
}

const [theme, setTheme] = createSignal<Theme>(readTheme());

function RedactButton() {
  const on = () => redaction() === "on";
  const label = () => (on() ? "Stop redacting" : "Redact names, rooms and paths");
  return (
    <button
      type="button"
      class="ghost icon-button"
      aria-label={label()}
      aria-pressed={on()}
      data-tip={label()}
      onClick={() => setRedaction(on() ? "off" : "on")}
    >
      <Icon name={on() ? "hide" : "show"} />
    </button>
  );
}

function ThemeButton() {
  const next = () => {
    const now = THEMES[(THEMES.indexOf(theme()) + 1) % THEMES.length] ?? "system";
    setTheme(now);
    applyTheme(now);
    try {
      localStorage.setItem("aqx-theme", now);
    } catch {
      return;
    }
  };
  const label = () => `Theme: ${theme()}`;
  return (
    <button type="button" class="ghost icon-button" aria-label={label()} data-tip={label()} onClick={next}>
      <Icon name={theme() === "system" ? "auto" : theme() === "light" ? "sun" : "moon"} />
    </button>
  );
}

function SessionMenu() {
  let menu!: HTMLDivElement;
  let button!: HTMLButtonElement;
  let release: (() => void) | undefined;
  onCleanup(() => release?.());
  const running = () => session()?.kind === "running";
  const restartable = () => {
    const s = session();
    return s?.kind === "incompatible" || (s?.kind === "running" && s.otherBuild);
  };
  const choose = (kind: "restart" | "end") => {
    menu.hidePopover();
    setDialog({ kind });
  };
  return (
    <Show when={running() || restartable()}>
      <button type="button" class="ghost" popovertarget="session-menu" aria-haspopup="true" ref={button}>
        <Icon name="power" />
        <span class="label">Session</span>
      </button>
      <div
        id="session-menu"
        class="menu"
        popover
        ref={menu}
        on:beforetoggle={(e: ToggleEvent) => {
          release?.();
          release = e.newState === "open" ? anchor(button, menu) : undefined;
        }}
      >
        <Show when={running() || restartable()}>
          <button type="button" onClick={() => choose("restart")}>
            {restartable() ? "Restart with the new build" : "Restart session"}
            <span class="muted small">Applies config changes. Online accounts log back in and rerun their scripts.</span>
          </button>
        </Show>
        <Show when={running()}>
          <button type="button" class="danger" onClick={() => choose("end")}>
            Log out everyone
            <span class="muted small">Ends the session</span>
          </button>
        </Show>
      </div>
    </Show>
  );
}

export function tone(a: Account) {
  const s = a.state;
  if (typeof s === "object" && "Offline" in s) return s.Offline.why ? "bad" : "off";
  if (s === "Stalled") return "warn";
  if (s === "Paused") return "paused";
  if (isRunning(a)) return "run";
  if (s === "Online") return "on";
  return "busy";
}

const [selecting, setSelecting] = createSignal(false);

const BRAILLE_DOTS = [
  [1.25, 2.75],
  [1.25, 7],
  [1.25, 11.25],
  [6.25, 2.75],
  [6.25, 7],
  [6.25, 11.25],
] as const;
type Dot = 0 | 1 | 2 | 3 | 4 | 5;
const BRAILLE_FRAMES: Dot[][] = [
  [0, 1, 3],
  [0, 3, 4],
  [0, 3, 4, 5],
  [3, 4, 5],
  [2, 3, 4, 5],
  [2, 4, 5],
  [1, 2, 5],
  [0, 1, 2, 5],
  [0, 1, 2],
  [0, 1, 2, 3],
  [0, 1, 2, 3, 4, 5],
];
const SPIN_STRIP = BRAILLE_FRAMES.flatMap((frame, i) =>
  frame.map((dot) => ({ x: i * 14 + BRAILLE_DOTS[dot][0], y: BRAILLE_DOTS[dot][1] })),
);
const SPIN_MS = 800;

function Mark(props: { tone: ReturnType<typeof tone> }) {
  return (
    <Show
      when={props.tone === "run" || props.tone === "busy"}
      fallback={
        <svg class={`mark m-${props.tone}`} viewBox="0 0 14 14" aria-hidden="true">
          <Switch>
            <Match when={props.tone === "on"}>
              <circle cx="4.5" cy="7" r="4.5" />
            </Match>
            <Match when={props.tone === "off"}>
              <circle class="line" cx="4.5" cy="7" r="3.75" />
            </Match>
            <Match when={props.tone === "bad"}>
              <path class="line" d="M0.75 2.75 8.25 11.25M8.25 2.75 0.75 11.25" />
            </Match>
            <Match when={props.tone === "paused"}>
              <path class="line" d="M0.75 2.5v9M5.75 2.5v9" />
            </Match>
            <Match when={props.tone === "warn"}>
              <path class="line bold" d="M1 2.5v5.5" />
              <circle cx="1" cy="11" r="1" />
            </Match>
          </Switch>
        </svg>
      }
    >
      <span class={`mark spin m-${props.tone}`} aria-hidden="true">
        <span style={{ "animation-delay": `${-(Number(document.timeline.currentTime ?? 0) % SPIN_MS)}ms` }}>
          <svg viewBox={`0 0 ${BRAILLE_FRAMES.length * 14} 14`}>
            <For each={SPIN_STRIP}>{(dot) => <circle cx={dot.x} cy={dot.y} r="1.25" />}</For>
          </svg>
        </span>
      </span>
    </Show>
  );
}

function stopSelecting() {
  setSelecting(false);
  setMarked(new Set<string>());
}

const HOLD_MS = 450;

function AccountList(props: { onOpen: (i: number | null) => void }) {
  let ref!: HTMLUListElement;
  let anchor: number | null = null;
  let held = false;
  let timer: number | undefined;
  createEffect(() => {
    if (!selecting()) anchor = null;
  });
  const tick = (i: number, e: MouseEvent) => {
    setSelecting(true);
    if (e.shiftKey && anchor !== null) markRange(anchor, i);
    else toggleMark(i);
    anchor = i;
  };
  let drag: { from: number; on: boolean; base: ReadonlySet<string>; moved: boolean } | null = null;
  let dragged = false;
  const grab = (i: number, e: PointerEvent) => {
    if (e.pointerType !== "mouse" || e.button !== 0 || e.shiftKey || e.metaKey || e.ctrlKey) return;
    drag = { from: i, on: !isMarked(i), base: marked(), moved: false };
  };
  const sweep = (i: number) => {
    if (!drag || (!drag.moved && i === drag.from)) return;
    if (!drag.moved) {
      drag.moved = true;
      setSelecting(true);
    }
    anchor = i;
    paint(drag.base, drag.from, i, drag.on);
  };
  const drop = () => {
    if (drag?.moved) {
      dragged = true;
      setTimeout(() => (dragged = false));
    }
    drag = null;
  };
  onMount(() => document.addEventListener("pointerup", drop));
  onCleanup(() => document.removeEventListener("pointerup", drop));
  const press = (i: number, e: MouseEvent) => {
    if (dragged) return;
    if (held) {
      held = false;
      return;
    }
    if (selecting() || e.shiftKey || e.metaKey || e.ctrlKey) tick(i, e);
    else {
      anchor = i;
      props.onOpen(i);
    }
  };
  const hold = (i: number, e: PointerEvent) => {
    if (e.pointerType === "mouse") return;
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      held = true;
      setSelecting(true);
      if (!isMarked(i)) toggleMark(i);
      anchor = i;
      navigator.vibrate?.(10);
    }, HOLD_MS);
  };
  const release = () => clearTimeout(timer);
  const rowFocus = (i: number) => ((selected() ?? ordered()[0]) === i ? 0 : -1);
  const firstOffline = () => {
    const order = ordered();
    const at = order.findIndex((i) => accounts[i] !== undefined && isOffline(accounts[i]));
    return at > 0 ? order[at] : undefined;
  };
  const offline = () => accounts.flatMap((a, i) => (isOffline(a) ? [i] : []));
  const loginLabel = () =>
    offline().length === accounts.length
      ? "Log in every account and run its script"
      : `Log in the ${offline().length} offline account${offline().length === 1 ? "" : "s"} and run ${offline().length === 1 ? "its script" : "their scripts"}`;
  let menu!: HTMLDivElement;
  let row: HTMLElement | undefined;
  const [menuFor, setMenuFor] = createSignal<Account | undefined>();
  const closeMenu = () => {
    if (menu.matches(":popover-open")) menu.hidePopover();
  };
  const outside = (e: PointerEvent) => {
    if (!(e.target instanceof Element) || menu.contains(e.target) || e.target.closest(".more")) return;
    closeMenu();
  };
  const watchMenu = () => {
    document.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", closeMenu);
    window.addEventListener("resize", closeMenu);
  };
  const unwatchMenu = () => {
    document.removeEventListener("pointerdown", outside, true);
    window.removeEventListener("blur", closeMenu);
    window.removeEventListener("resize", closeMenu);
    setMenuFor(undefined);
  };
  onCleanup(unwatchMenu);
  const openMenu = (i: number, from: HTMLElement, x: number, y: number, align: "start" | "end") => {
    row = from.closest("li")?.querySelector<HTMLElement>("[data-nav]") ?? from;
    setMenuFor(accounts[i]);
    if (!menu.matches(":popover-open")) menu.showPopover();
    const left = align === "end" ? x - menu.offsetWidth : x;
    menu.style.left = `${Math.max(8, Math.min(left, innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, innerHeight - menu.offsetHeight - 8))}px`;
    menu.querySelector("button")?.focus();
  };
  const format = createFormatForDisplay();
  const modHeld = createHotkeyHint(JUMPS[0]);
  const [revealed, setRevealed] = createSignal(false);
  createEffect(() => {
    if (!modHeld()) return setRevealed(false);
    const timer = setTimeout(() => setRevealed(true), REVEAL_MS);
    onCleanup(() => clearTimeout(timer));
  });
  const picks: [string, (a: Account) => boolean][] = [
    ["All", () => true],
    ["Running", isRunning],
    ["Offline", isOffline],
    ["None", () => false],
  ];
  return (
    <nav class="accounts" classList={{ selecting: selecting() }} aria-label="Accounts">
      <div class="accounts-head">
        <Show
          when={selecting()}
          fallback={
            <button
              type="button"
              class="ghost overview"
              aria-current={selected() === null ? "page" : undefined}
              onClick={() => props.onOpen(null)}
            >
              <Icon name="users" />
              Overview
            </button>
          }
        >
          <label class="check small">
            <input
              type="checkbox"
              checked={accounts.length > 0 && chosen().length === accounts.length}
              ref={(el) =>
                createEffect(() => (el.indeterminate = chosen().length > 0 && chosen().length < accounts.length))
              }
              onChange={() => markAll()}
            />
            {chosen().length} selected
          </label>
        </Show>
        <span class="grow" />
        <Show when={!selecting() && offline().length > 0}>
          <button
            type="button"
            class="ghost"
            data-tip={loginLabel()}
            onClick={() => act("login", "logging in", offline())}
          >
            Log in {offline().length === accounts.length ? "all" : offline().length}
          </button>
        </Show>
        <button
          type="button"
          class="ghost"
          aria-pressed={selecting()}
          onClick={() => (selecting() ? stopSelecting() : setSelecting(true))}
        >
          {selecting() ? "Done" : "Select"}
        </button>
      </div>
      <Show when={selecting()}>
        <div class="picks" role="group" aria-label="Select accounts by state">
          <For each={picks}>
            {([label, test]) => (
              <button type="button" class="ghost small" onClick={() => markWhere(test)}>
                {label}
              </button>
            )}
          </For>
        </div>
      </Show>
      <ul
        ref={ref}
        onKeyDown={(e) => {
          const down = e.key === "ArrowDown";
          if (!e.shiftKey || (!down && e.key !== "ArrowUp")) return arrows(e, ref, "button[data-nav]");
          const row = e.target instanceof HTMLElement ? e.target.closest<HTMLElement>("[data-index]") : null;
          if (!row) return;
          e.preventDefault();
          const at = Number(row.dataset.index);
          const order = ordered();
          const to = order[Math.min(order.length - 1, Math.max(0, order.indexOf(at) + (down ? 1 : -1)))] ?? at;
          setSelecting(true);
          anchor ??= at;
          markRange(anchor, to);
          ref.querySelector<HTMLElement>(`[data-index="${to}"]`)?.focus();
        }}
      >
        <For each={ordered()}>
          {(index) => (
            <Show when={accounts[index]}>
              {(a) => {
            const i = () => index;
            return (
            <>
            <Show when={!selecting() && firstOffline() === index}>
              <li class="divider" role="separator" />
            </Show>
            <li
              onPointerEnter={() => sweep(i())}
              classList={{
                current: !selecting() && selected() === i(),
                marked: selecting() && isMarked(i()),
                "menu-for": menuFor() === a(),
              }}
            >
              <button
                type="button"
                data-nav
                data-index={i()}
                class="row"
                role={selecting() ? "checkbox" : undefined}
                aria-checked={selecting() ? isMarked(i()) : undefined}
                aria-current={!selecting() && selected() === i() ? "page" : undefined}
                tabindex={selecting() ? (i() === ordered()[0] ? 0 : -1) : rowFocus(i())}
                onClick={(e) => press(i(), e)}
                onPointerDown={(e) => {
                  grab(i(), e);
                  hold(i(), e);
                }}
                onPointerUp={release}
                onPointerLeave={release}
                onPointerCancel={release}
                onContextMenu={(e) => {
                  e.preventDefault();
                  if (selecting() || held || !(e.currentTarget instanceof HTMLElement)) return;
                  const at = e.currentTarget.getBoundingClientRect();
                  if (e.clientX === 0 && e.clientY === 0) openMenu(i(), e.currentTarget, at.left + 8, at.bottom, "start");
                  else openMenu(i(), e.currentTarget, e.clientX, e.clientY, "start");
                }}
              >
                <Show
                  when={selecting()}
                  fallback={<Mark tone={tone(a())} />}
                >
                  <span class="mark">
                    <span class="tick" classList={{ on: isMarked(i()) }} aria-hidden="true" />
                  </span>
                </Show>
                <span class="name">{hide(a().name)}</span>
                <Show when={revealed() && JUMPS[i()]}>
                  {(jump) => (
                    <kbd class="jump" aria-hidden="true">
                      {format(jump())}
                    </kbd>
                  )}
                </Show>
                <span class={`state s-${tone(a())}`} title={hide(reason(a()) ?? "") || undefined}>
                  {stateLabel(a())}
                </span>
              </button>
                <button
                  type="button"
                  class="ghost icon-button more"
                  tabindex="-1"
                  disabled={selecting()}
                  aria-hidden={selecting() ? "true" : undefined}
                  aria-label={`More for ${hide(a().name)}`}
                  aria-haspopup="true"
                  aria-expanded={menuFor() === a()}
                  data-tip="More"
                  onClick={(e) => {
                    if (menuFor() === a()) return closeMenu();
                    const at = e.currentTarget.getBoundingClientRect();
                    openMenu(i(), e.currentTarget, at.right, at.bottom + 2, "end");
                  }}
                >
                  <Icon name="more" />
                </button>
            </li>
            </>
            );
              }}
            </Show>
          )}
        </For>
        <Show when={!selecting()}>
          <li class="add">
            <button
              type="button"
              data-nav
              tabindex="-1"
              onClick={() => setDialog({ kind: "config", start: { kind: "new" } })}
            >
              <span class="mark">
                <Icon name="add" />
              </span>
              <span class="name">Add account</span>
            </button>
          </li>
        </Show>
      </ul>
      <Show when={selecting()}>
        <Selection />
      </Show>
      <div
        id="account-menu"
        class="menu context"
        popover="manual"
        ref={menu}
        on:toggle={(e: ToggleEvent) => (e.newState === "open" ? watchMenu() : unwatchMenu())}
        onKeyDown={(e) => {
          if (e.key !== "Escape") return;
          e.stopPropagation();
          closeMenu();
          row?.focus();
        }}
        onFocusOut={(e) => {
          if (!(e.relatedTarget instanceof Node && menu.contains(e.relatedTarget))) closeMenu();
        }}
        aria-label={`Actions for ${menuFor()?.name ?? ""}`}
      >
        <button
          type="button"
          onClick={() => {
            const user = menuFor()?.user;
            row?.focus();
            closeMenu();
            setDialog({ kind: "config", start: user === undefined ? { kind: "list" } : { kind: "account", user } });
          }}
        >
          Edit config…
        </button>
      </div>
    </nav>
  );
}

function Dashboard(props: { onOpen: (i: number) => void }) {
  return (
    <section aria-labelledby="dashboard-title" class="dashboard">
      <h2 id="dashboard-title" class="visually-hidden" tabindex="-1">
        Every account
      </h2>
      <ul class="cards">
        <For each={ordered()}>
          {(i) => (
            <Show when={accounts[i]}>
              {(account) => {
                const a = account();
                return (
            <li>
              <button
                type="button"
                class="card"
                classList={{ "c-warn": a.state === "Stalled", "c-bad": tone(a) === "bad" || a.game?.dead === true }}
                aria-label={`${hide(a.name)}, ${a.game?.dead ? "dead" : stateLabel(a)}`}
                onClick={() => props.onOpen(i)}
              >
                <span class="card-head">
                  <strong>{hide(a.name)}</strong>
                  <span class={`state s-${tone(a)}`}>{stateLabel(a)}</span>
                </span>
                <Show when={reason(a)}>{(why) => <span class="bad ellipsis">{hide(why())}</span>}</Show>
                <Show
                  when={a.game}
                  fallback={
                    <Show when={a.server}>
                      <span class="muted">
                        <ServerName name={a.server} />
                      </span>
                    </Show>
                  }
                >
                  {(g) => (
                    <>
                      <span class="ellipsis">
                        <ServerName name={a.server} /> · {hide(g().area)} {g().cell}
                      </span>
                      <span class="muted ellipsis">
                        lv {g().level} {g().class}
                      </span>
                      <span class="muted ellipsis">
                        hp {number.format(g().hp)}/{number.format(g().max_hp)}
                        <Show when={g().dead}>
                          <span class="badge bad">dead</span>
                        </Show>
                      </span>
                    </>
                  )}
                </Show>
                <span class="muted ellipsis">
                  {a.script ? a.script.split("/").pop() : "no script"}
                  {a.status ? ` · ${hide(a.status)}` : ""}
                </span>
                <Show when={session()?.kind === "none" && lastLines(a.user).at(-1)}>
                  {(line) => <span class="muted ellipsis">{lineText(line())}</span>}
                </Show>
              </button>
            </li>
                );
              }}
            </Show>
          )}
        </For>
      </ul>
    </section>
  );
}

type Bulk = {
  icon: () => IconName;
  label: () => string;
  fits: (a: Account) => boolean;
  none: string;
  run: (on: number[]) => void;
  danger?: boolean;
  loginMenu?: true;
};

function Selection() {
  const count = (n: number) => `${n} account${n === 1 ? "" : "s"}`;
  const confirm = (title: string, body: string, label: string, run: () => void) =>
    setDialog({ kind: "confirm", title, body, label, danger: true, run });
  const ask = (verb: string, body: string, run: (on: number[]) => void) => (on: number[]) =>
    on.length > 1 ? confirm(`${verb} ${count(on.length)}?`, body, verb, () => run(on)) : run(on);
  const paused = (on: number[]) => on.length > 0 && on.every((i) => accounts[i]?.state === "Paused");
  const skua = (a: Account) => a.script?.endsWith(".cs") ?? false;
  const pausable = (a: Account) => isRunning(a) && !skua(a);
  const bulk: Bulk[] = [
    {
      icon: () => "login",
      label: () => "Log in",
      fits: isOffline,
      none: "None of them are offline",
      run: (on) => act("login", "logging in", on),
      loginMenu: true,
    },
    {
      icon: () => "play",
      label: () => "Run…",
      fits: () => true,
      none: "",
      run: (on) => openPicker(on),
    },
    {
      icon: () => "stop",
      label: () => "Stop",
      fits: isRunning,
      none: "None of them are running a script",
      run: ask("Stop", "Their scripts stop and they leave combat. They stay logged in.", (on) =>
        act("stop", "stopping", on),
      ),
    },
    {
      icon: () => "restart",
      label: () => "Restart",
      fits: (a) => a.script !== null,
      none: "None of them have run a script yet",
      run: ask("Restart", "Their scripts stop and start again from the top.", (on) =>
        act("restart", "restarting", on),
      ),
    },
    {
      icon: () => (paused(fitting(pausable)) ? "play" : "pause"),
      label: () => (paused(fitting(pausable)) ? "Resume" : "Pause"),
      fits: pausable,
      none: "None of them are running a script that can pause",
      run: (on) => pauseOrResume(on),
    },
    {
      icon: () => "server",
      label: () => "Server…",
      fits: isOnline,
      none: "None of them are online",
      run: (on) => openServers(on),
    },
    {
      icon: () => "logout",
      label: () => "Log out",
      fits: (a) => !isOffline(a),
      none: "None of them are online",
      danger: true,
      run: logOut,
    },
  ];
  function fitting(fits: (a: Account) => boolean) {
    return chosen().filter((i) => {
      const a = accounts[i];
      return a !== undefined && fits(a);
    });
  }
  return (
    <div class="selection" role="toolbar" aria-label={`Actions for ${count(chosen().length)}`}>
      <span class="selection-count">{chosen().length} selected</span>
      <For each={bulk}>
        {(b) => {
          const on = () => fitting(b.fits);
          const blocked = () => (chosen().length === 0 ? "Select some accounts first" : on().length === 0 ? b.none : "");
          const partial = () => on().length > 0 && on().length < chosen().length;
          const button = (
            <button
              type="button"
              class="action"
              classList={{ danger: b.danger === true }}
              aria-disabled={blocked() ? "true" : undefined}
              data-tip={blocked() || (partial() ? `${b.label().replace("…", "")} the ${count(on().length)} it applies to` : undefined)}
              onClick={() => {
                if (!blocked()) b.run(on());
                else if (!canHover()) say(blocked());
              }}
            >
              <Icon name={b.icon()} />
              <span class="label">
                {b.label()}
                <Show when={partial()}>
                  <span class="muted"> {on().length}</span>
                </Show>
              </span>
            </button>
          );
          if (!b.loginMenu) return button;
          return (
            <div class="login-split">
              {button}
              <Show when={!blocked()}>
                <LoginMenu on={on()} />
              </Show>
            </div>
          );
        }}
      </For>
    </div>
  );
}

function Toasts() {
  let layer!: HTMLDivElement;
  const { toasts } = useSonner();
  onMount(() => layer.showPopover());
  createEffect(
    on(
      () => toasts().at(-1)?.id,
      () => {
        layer.hidePopover();
        layer.showPopover();
      },
      { defer: true },
    ),
  );
  return (
    <div class="toast-layer" popover="manual" ref={layer}>
      <Toaster theme={theme()} position="bottom-right" richColors />
    </div>
  );
}
