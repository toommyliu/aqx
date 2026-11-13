import { createVirtualizer } from "@tanstack/solid-virtual";
import {
  For,
  type JSX,
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  on,
  onCleanup,
  onMount,
} from "solid-js";
import { type AppHotkeyId } from "@tanstack/hotkeys";
import { createFormatForDisplay, useAppHotkeys } from "@tanstack/solid-hotkeys";
import { pauseOrResume, recordOrStop, recordable, tone } from "./App";
import { logOut, openPicker, openQueuePicker, openQueuedOptions, openServers, openScriptOptions } from "./Dialogs";
import {
  VIEWS,
  type View,
  act,
  ended,
  finder,
  gameLines,
  isPacket,
  lastLines,
  lineText,
  session,
  log,
  logLoading,
  fail,
  report,
  say,
  search,
  setGameLines,
  setSearch,
  show,
  shows,
  targetsOf,
  view,
  hide,
} from "./store";
import { Found, Icon, type IconName, KeyCaps, ServerName, Since, amount, anchor, canHover, clock, number } from "./ui";
import {
  type Account,
  type Channel,
  type Game,
  type Item,
  type Line,
  type Registered,
  type Stack,
  api,
  isOffline,
  isOnline,
  isRunning,
  stateLabel,
} from "./wire";

const [details, setDetails] = createSignal(false);
const [copied, setCopied] = createSignal<number | null>(null);
let uncopy: ReturnType<typeof setTimeout> | undefined;

export function AccountPanel(props: { account: Account; index: number }) {
  const a = () => props.account;
  const views = createMemo(() =>
    VIEWS.filter((v) => v.id === "log" || (v.id === "packets" ? a().recording !== null : a().game !== null)),
  );
  const shown = () => (views().some((v) => v.id === view()) ? view() : "log");
  return (
    <section class="account" classList={{ details: details() }} aria-labelledby="account-name">
      <Header account={a()} index={props.index} />
      <Actions account={a()} index={props.index} />
      <Show when={a().queue.length > 0}>
        <UpNext account={a()} index={props.index} />
      </Show>
      <Tabs views={views()} game={a().game} shown={shown()} />
      <div class="panel" id={`panel-${shown()}`} role="tabpanel" aria-labelledby={`tab-${shown()}`}>
        <Switch>
          <Match when={shown() === "log"}>
            <Log user={a().user} />
          </Match>
          <Match when={shown() === "packets"}>
            <Log user={a().user} packets />
          </Match>
          <Match when={a().game}>
            {(g) => <Lists game={g()} view={shown()} index={props.index} registered={a().registered} />}
          </Match>
        </Switch>
      </div>
    </section>
  );
}

function Header(props: { account: Account; index: number }) {
  const a = () => props.account;
  const why = () => {
    const s = a().state;
    if (typeof s === "object" && "Offline" in s) return s.Offline.why;
    if (typeof s === "object" && "Reconnecting" in s) return s.Reconnecting;
    return null;
  };
  const recordBlocked = () => !recordable(a()) && "Log in first";
  const upSince = createMemo(() => {
    const up = a().up;
    return up ? Date.now() - up.secs * 1000 : null;
  });
  return (
    <div class="account-head">
      <div class="line">
        <h2 id="account-name" tabindex="-1">{hide(a().name)}</h2>
        <span class={`state head-state s-${tone(a())}`}>{stateLabel(a())}</span>
        <Show when={a().server}>
          <span>
            <ServerName name={a().server} />
            <Show when={a().game}>
              {(g) => (
                <>
                  {" · "}
                  {hide(g().area)} <span class="muted">{g().cell}</span>
                </>
              )}
            </Show>
          </span>
        </Show>
        <Show when={upSince()}>
          {(at) => (
            <span class="muted">
              <Since at={at()} prefix="up" />
            </span>
          )}
        </Show>
        <Show when={why()}>{(w) => <span class="bad">{hide(w())}</span>}</Show>
        <button
          type="button"
          class="ghost small recorder"
          aria-pressed={a().recording !== null}
          aria-label="Record packets"
          aria-disabled={recordBlocked() ? "true" : undefined}
          data-tip={a().recording ? "Stop recording" : "Record packets"}
          onClick={() => {
            const blocked = recordBlocked();
            if (!blocked) void recordOrStop([props.index]);
            else if (!canHover()) say(blocked);
          }}
        >
          <Show when={a().recording} fallback={<Icon name="record" />}>
            <span class="rec" aria-hidden="true" />
            rec
          </Show>
        </button>
      </div>
      <Show when={a().game}>{(g) => <Vitals game={g()} account={a()} />}</Show>
      <div class="line small">
        <span class="muted">script</span>
        <span class="ellipsis" data-tip={a().script ? hide(a().script ?? "") : undefined}>
          {a().script?.split("/").pop() ?? "none"}
        </span>
        <Show when={a().status}>
          <span class="muted ellipsis">· {hide(a().status ?? "")}</span>
        </Show>
        <Show when={a().game}>
          <button
            type="button"
            class="ghost small details-toggle"
            aria-expanded={details()}
            onClick={() => setDetails(!details())}
          >
            {details() ? "Less" : "More"}
          </button>
        </Show>
      </div>
    </div>
  );
}

function Meter(props: { label: string; value: number; max: number; kind: "hp" | "mp" }) {
  const max = () => Math.max(1, props.max);
  const health = () => props.kind === "hp";
  return (
    <span class="meter-wrap">
      <span class="muted">{props.label}</span>
      <meter
        class={props.kind}
        min={0}
        max={max()}
        value={props.value}
        low={health() ? max() * 0.25 : undefined}
        high={health() ? max() * 0.5 : undefined}
        optimum={health() ? max() : undefined}
        aria-label={props.label}
      />
      <span class="num">
        {number.format(props.value)}/{number.format(props.max)}
      </span>
    </span>
  );
}

function Vitals(props: { game: Game; account: Account }) {
  const g = () => props.game;
  return (
    <>
      <div class="line small">
        <Meter label="hp" value={g().hp} max={g().max_hp} kind="hp" />
        <Meter label="mp" value={g().mp} max={g().max_mp} kind="mp" />
        <span>
          lv {g().level} {g().class} <span class="muted">r{g().class_rank}</span>
        </span>
        <Show when={g().dead}>
          <span class="badge bad">dead</span>
        </Show>
        <Show when={g().away}>
          <span class="badge">afk</span>
        </Show>
      </div>
      <div class="line small more muted num">
        <span>
          {number.format(g().gold)} gold
          <Show when={props.account.gold_gained}>
            {(gained) => ` (${gained() >= 0 ? "+" : ""}${number.format(gained())})`}
          </Show>
        </span>
        <span>{amount(props.account.stats.kills, "kill")}</span>
        <span>{amount(props.account.stats.deaths, "death")}</span>
        <span>{amount(props.account.stats.drops, "drop")}</span>
      </div>
      <div class="line small more">
        <Show
          when={g().target}
          fallback={
            <span>
              <span class="muted">target</span> <span class="muted">none</span>
            </span>
          }
        >
          {(t) => (
            <span>
              <span class="muted">target</span> {hide(t().name)}{" "}
              <span class="num">
                {number.format(t().hp)}/{number.format(t().max_hp)}
              </span>
            </span>
          )}
        </Show>
      </div>
      <div class="line small more">
        <span class="skills">
          <span class="muted">skills</span>
          <For each={g().skills}>{(skill, i) => <Skill n={i() + 1} skill={skill} />}</For>
        </span>
      </div>
      <Show when={g().auras.length > 0}>
        <div class="line small more">
          <span class="auras">
            <span class="muted">auras</span>{" "}
            {g()
              .auras.map((a) => (a.stacks > 1 ? `${a.name} ×${a.stacks}` : a.name))
              .join(", ")}
          </span>
        </div>
      </Show>
    </>
  );
}

function Action(props: {
  icon: IconName;
  label: string;
  keys: AppHotkeyId;
  onClick: () => void;
  blocked?: string | false;
}) {
  const hotkeys = useAppHotkeys();
  const format = createFormatForDisplay();
  const why = createUniqueId();
  const key = () => {
    const hotkey = hotkeys()[props.keys].hotkey;
    return typeof hotkey === "string" ? hotkey : undefined;
  };
  const tip = () => {
    if (props.blocked) return props.blocked;
    const k = key();
    return k ? `${props.label} (${format(k)})` : props.label;
  };
  return (
    <button
      type="button"
      class="action"
      aria-keyshortcuts={key()}
      data-tip={tip()}
      aria-disabled={props.blocked ? "true" : undefined}
      aria-describedby={props.blocked ? why : undefined}
      onClick={() => {
        if (!props.blocked) props.onClick();
        else if (!canHover()) say(props.blocked);
      }}
    >
      <Icon name={props.icon} />
      <span class="label">{props.label}</span>
      <Show when={props.blocked}>
        {(reason) => (
          <span id={why} class="visually-hidden">
            {reason()}
          </span>
        )}
      </Show>
    </button>
  );
}

type MenuItem = { title: string; note: string; keys?: AppHotkeyId; run: () => void };

function SplitMenu(props: { label: string; item: MenuItem }) {
  let menu!: HTMLDivElement;
  let button!: HTMLButtonElement;
  let release: (() => void) | undefined;
  onCleanup(() => release?.());
  const id = createUniqueId();
  const hotkeys = useAppHotkeys();
  const format = createFormatForDisplay();
  const key = () => {
    const keys = props.item.keys;
    const hotkey = keys === undefined ? undefined : hotkeys()[keys].hotkey;
    return typeof hotkey === "string" ? hotkey : undefined;
  };
  return (
    <>
      <button
        type="button"
        class="action expand"
        popovertarget={id}
        aria-haspopup="true"
        aria-label={props.label}
        data-tip={props.label}
        ref={button}
      >
        <Icon name="expand" />
      </button>
      <div
        id={id}
        class="menu"
        popover
        ref={menu}
        on:beforetoggle={(e: ToggleEvent) => {
          release?.();
          release = e.newState === "open" ? anchor(button, menu, "bottom-start") : undefined;
        }}
        on:toggle={(e: ToggleEvent) => {
          if (e.newState === "open") requestAnimationFrame(() => menu.querySelector("button")?.focus());
        }}
      >
        <button
          type="button"
          aria-keyshortcuts={key()}
          onClick={() => {
            button.parentElement?.querySelector("button")?.focus();
            menu.hidePopover();
            props.item.run();
          }}
        >
          {props.item.title}
          <span class="muted small">
            {props.item.note}
            <Show when={key()}>{(k) => ` (${format(k())})`}</Show>
          </span>
        </button>
      </div>
    </>
  );
}

export function LoginMenu(props: { on: number[] }) {
  return (
    <SplitMenu
      label="More ways to log in"
      item={{
        title: "Log in without the script",
        note: "Stays idle until you run a script",
        keys: "account.loginOnly",
        run: () => void act("login_only", "logging in", props.on),
      }}
    />
  );
}

export function RunMenu(props: { on: number[] }) {
  return (
    <SplitMenu
      label="More ways to run"
      item={{
        title: "Add to queue…",
        note: "Runs after the current script finishes",
        run: () => openQueuePicker(props.on),
      }}
    />
  );
}

function Actions(props: { account: Account; index: number }) {
  const a = () => props.account;
  const on = () => [props.index];
  return (
    <div class="actions" role="group" aria-label={`Actions for ${hide(a().name)}`}>
      <div class="login-split">
        <Action
          icon={isOffline(a()) ? "login" : "logout"}
          label={isOffline(a()) ? "Log in" : "Log out"}
          keys={isOffline(a()) ? "account.login" : "account.logout"}
          onClick={() => (isOffline(a()) ? act("login", "logging in", on()) : logOut(on()))}
        />
        <Show when={isOffline(a())}>
          <LoginMenu on={on()} />
        </Show>
      </div>
      <div class="login-split">
        <Action icon="play" label="Run…" keys="account.run" onClick={() => openPicker(on())} />
        <RunMenu on={on()} />
      </div>
      <Show when={isRunning(a())}>
        <Action
          icon="stop"
          label="Stop"
          keys="account.stop"
          onClick={() => {
            document.querySelector<HTMLElement>('.actions [data-tip^="Run"]')?.focus();
            void act("stop", "stopping", on());
          }}
        />
      </Show>
      <Action icon="restart" label="Restart script" keys="account.restart" onClick={() => act("restart", "restarting", on())} blocked={!a().script && "No script has run yet"} />
      <Action
        icon={a().state === "Paused" ? "play" : "pause"}
        label={a().state === "Paused" ? "Resume" : "Pause"}
        keys="account.pause"
        onClick={() => pauseOrResume(on())}
        blocked={
          a().script?.endsWith(".cs") ? "Skua scripts can't pause" : !isRunning(a()) && "No script is running"
        }
      />
      <Show when={a().script}>
        {(script) => (
          <Action
            icon="options"
            label={script().endsWith(".cs") ? "Options…" : "Args…"}
            keys="account.options"
            onClick={() => void openScriptOptions(on())}
          />
        )}
      </Show>
      <Action icon="server" label="Server…" keys="account.server" onClick={() => openServers(on())} blocked={!isOnline(a()) && "Log in first"} />
    </div>
  );
}

function shownArgs(args: [string, string][]): string {
  return args.map(([k, v]) => `${k.replace(/^Options:/, "")}=${v}`).join(", ");
}

function UpNext(props: { account: Account; index: number }) {
  const a = () => props.account;
  const on = () => [props.index];
  const last = () => a().queue.length - 1;
  const move = (id: number, to: number) => void act({ move_queued: { id, to } }, "moved it", on());
  return (
    <section class="up-next" aria-labelledby="up-next-title">
      <div class="line small">
        <h3 id="up-next-title" class="muted">
          Up next
        </h3>
        <button type="button" class="ghost small" onClick={() => act("next", "running the next script", on())}>
          {isRunning(a()) ? "Skip to next" : "Start"}
        </button>
        <button type="button" class="ghost small" onClick={() => act("clear_queue", "emptied the queue", on())}>
          Clear
        </button>
      </div>
      <ol>
        <For each={a().queue}>
          {(q, i) => {
            const name = () => q.script.path.split("/").pop() ?? q.script.path;
            return (
              <li>
                <span class="ellipsis" data-tip={hide(q.script.path)}>
                  {name()}
                </span>
                <Show when={q.script.args.length > 0}>
                  <span class="muted ellipsis">{hide(shownArgs(q.script.args))}</span>
                </Show>
                <span class="queued-actions">
                  <Show when={q.script.path.endsWith(".cs")}>
                    <button
                      type="button"
                      class="icon-button"
                      aria-label={`Options for ${name()}`}
                      data-tip="Options for this run"
                      onClick={() => openQueuedOptions(props.index, q.id)}
                    >
                      <Icon name="options" />
                    </button>
                  </Show>
                  <button
                    type="button"
                    class="icon-button"
                    aria-label={`Move ${name()} up`}
                    disabled={i() === 0}
                    onClick={() => move(q.id, i() - 1)}
                  >
                    <Icon name="up" />
                  </button>
                  <button
                    type="button"
                    class="icon-button"
                    aria-label={`Move ${name()} down`}
                    disabled={i() === last()}
                    onClick={() => move(q.id, i() + 1)}
                  >
                    <Icon name="down" />
                  </button>
                  <button
                    type="button"
                    class="icon-button"
                    aria-label={`Remove ${name()} from the queue`}
                    onClick={() => act({ unqueue: q.id }, "removed it from the queue", on())}
                  >
                    <Icon name="close" />
                  </button>
                </span>
              </li>
            );
          }}
        </For>
      </ol>
    </section>
  );
}

function count(game: Game | null, id: View): number | undefined {
  if (!game) return undefined;
  const counts: Record<View, number | undefined> = {
    log: undefined,
    players: game.players.length,
    drops: game.drops.length,
    quests: game.quests.length,
    temp: game.temp.length,
    inventory: game.inventory.length,
    bank: game.bank?.length,
    factions: game.factions.length,
    packets: undefined,
  };
  return counts[id];
}

function Tabs(props: { views: readonly { id: View; label: string }[]; game: Game | null; shown: View }) {
  let ref!: HTMLDivElement;
  const move = (e: KeyboardEvent) => {
    const at = props.views.findIndex((v) => v.id === props.shown);
    const to =
      e.key === "ArrowRight"
        ? (at + 1) % props.views.length
        : e.key === "ArrowLeft"
          ? (at - 1 + props.views.length) % props.views.length
          : e.key === "Home"
            ? 0
            : e.key === "End"
              ? props.views.length - 1
              : -1;
    const next = props.views[to];
    if (!next) return;
    e.preventDefault();
    show(next.id);
    ref.querySelector<HTMLElement>(`#tab-${next.id}`)?.focus();
  };
  const [left, setLeft] = createSignal(false);
  const [right, setRight] = createSignal(false);
  const edges = () => {
    setLeft(ref.scrollLeft > 1);
    setRight(ref.scrollLeft + ref.clientWidth < ref.scrollWidth - 1);
  };
  const reveal = () =>
    ref.querySelector(`#tab-${props.shown}`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  let visible = false;
  const watch = new ResizeObserver(() => {
    const now = ref.clientWidth > 0;
    if (now && !visible) reveal();
    visible = now;
    edges();
  });
  onMount(() => watch.observe(ref));
  onCleanup(() => watch.disconnect());
  createEffect(on(() => props.shown, reveal));
  return (
    <div
      class="tabs"
      classList={{ "more-left": left(), "more-right": right() }}
      role="tablist"
      aria-label="Views"
      ref={ref}
      onKeyDown={move}
      onScroll={edges}
    >
      <For each={props.views}>
        {(v) => (
          <button
            type="button"
            role="tab"
            ref={(el) => watch.observe(el)}
            id={`tab-${v.id}`}
            aria-selected={props.shown === v.id}
            aria-controls={`panel-${v.id}`}
            tabindex={props.shown === v.id ? 0 : -1}
            onClick={() => show(v.id)}
          >
            {v.label}
            <Show when={count(props.game, v.id)}>{(n) => <span class="count">{n()}</span>}</Show>
          </button>
        )}
      </For>
    </div>
  );
}

function Search(props: { label: string }) {
  const hotkeys = useAppHotkeys();
  const key = () => {
    const hotkey = hotkeys()["search.focus"].hotkey;
    return typeof hotkey === "string" ? hotkey : undefined;
  };
  return (
    <div class="search">
    <input
      id="search"
      type="search"
      aria-label={props.label}
      aria-keyshortcuts={key()}
      placeholder="Search"
      value={search()}
      onInput={(e) => setSearch(e.currentTarget.value)}
      onKeyDown={(e) => {
        if (e.key === "Escape" && search()) {
          e.stopPropagation();
          setSearch("");
        } else if (e.key === "Enter" || e.key === "Escape") {
          e.currentTarget.blur();
        }
      }}
    />
      <Show when={key()}>{(k) => <KeyCaps hotkey={k()} />}</Show>
    </div>
  );
}

async function copyPacket(line: Line) {
  try {
    await navigator.clipboard.writeText(line.text);
    setCopied(line.seq);
    clearTimeout(uncopy);
    uncopy = setTimeout(() => setCopied(null), 1500);
  } catch (e) {
    fail(e instanceof Error ? e.message : String(e));
  }
}

function Log(props: { user: string; packets?: boolean }) {
  let ref!: HTMLDivElement;
  const [stuck, setStuck] = createSignal(true);
  let lastTop = 0;
  const past = () => (session()?.kind === "none" ? ended() : null);
  const find = createMemo(() => finder(search()));
  const lines = createMemo(() => {
    const query = find();
    const source = past() ? lastLines(props.user) : log();
    const wanted = (channel: Channel) => (props.packets ? isPacket(channel) : shows(gameLines(), channel));
    return source.filter((l) => wanted(l.channel) && (!query || query.test(lineText(l))));
  });
  const virtual = createVirtualizer({
    get count() {
      return lines().length;
    },
    getScrollElement: () => ref,
    estimateSize: () => 18,
    overscan: 20,
  });
  const newest = () => {
    if (lines().length > 0) virtual.scrollToIndex(lines().length - 1, { align: "end" });
  };
  createEffect(
    on(lines, () => {
      if (stuck()) requestAnimationFrame(newest);
    }),
  );

  return (
    <>
      <div class="toolbar">
        <Show when={past()}>
          {(e) => <span class="muted small">The session ended at {clock(e().endedMs)}. These are its last lines.</span>}
        </Show>
        <Search label={props.packets ? "Search the packets" : "Search the log"} />
        <span class="visually-hidden" role="status">
          {props.packets && copied() !== null ? "Copied the packet" : ""}
        </span>
        <Show when={!props.packets}>
          <button
            type="button"
            class="toggle"
            aria-pressed={gameLines() === "shown"}
            onClick={() => setGameLines(gameLines() === "shown" ? "hidden" : "shown")}
          >
            Verbose
          </button>
        </Show>
      </div>
      <div
        class="log"
        ref={ref}
        tabindex="0"
        role="log"
        aria-live="off"
        aria-label="Log"
        onScroll={() => {
          if (ref.scrollHeight - ref.scrollTop - ref.clientHeight <= 1) setStuck(true);
          else if (ref.scrollTop < lastTop) setStuck(false);
          lastTop = ref.scrollTop;
        }}
      >
        <Show
          when={lines().length > 0}
          fallback={
            <Show when={past() || !logLoading()}>
              <p class="muted empty">{search() ? "No matching lines" : props.packets ? "No packets yet" : "Nothing logged yet"}</p>
            </Show>
          }
        >
          <div class="log-body" style={{ height: `${virtual.getTotalSize()}px` }}>
            <For each={virtual.getVirtualItems()}>
              {(v) => (
                  <Show when={lines()[v.index]}>
                    {(line) => (
                      <div
                        class={`ln ch-${line().channel}`}
                        data-index={v.index}
                        ref={(el) => queueMicrotask(() => virtual.measureElement(el))}
                        style={{ transform: `translateY(${v.start}px)` }}
                      >
                        <span class="t">{clock(line().at_ms)}</span>
                        <span class="c">{line().channel}</span>
                        <span class="x">
                          <Found text={lineText(line())} find={find()} />
                        </span>
                        <Show when={props.packets}>
                          <button
                            type="button"
                            class="ghost icon-button copy"
                            classList={{ copied: copied() === line().seq }}
                            aria-label="Copy the packet"
                            data-tip="Copy"
                            onClick={() => void copyPacket(line())}
                          >
                            <Icon name={copied() === line().seq ? "check" : "copy"} />
                          </button>
                        </Show>
                      </div>
                    )}
                  </Show>
              )}
            </For>
          </div>
        </Show>
      </div>
      <Show when={!stuck()}>
        <button
          type="button"
          class="newest"
          onClick={() => {
            setStuck(true);
            newest();
          }}
        >
          <Icon name="down" />
          Newest
        </button>
      </Show>
    </>
  );
}

const matches = (find: RegExp | null, name: string) => !find || find.test(name);

function Table(props: { label: string; head: JSX.Element[]; children: JSX.Element; empty: boolean }) {
  return (
    <Show when={!props.empty} fallback={<p class="muted empty">{search() ? "Nothing matches" : `No ${props.label.toLowerCase()}`}</p>}>
      <div class="scroll">
        <table class="list">
          <caption class="visually-hidden">{props.label}</caption>
          <thead>
            <tr>
              <For each={props.head}>{(h, i) => <th scope="col" classList={{ num: i() > 0 }}>{h}</th>}</For>
            </tr>
          </thead>
          <tbody>{props.children}</tbody>
        </table>
      </div>
    </Show>
  );
}

function Stacks(props: { label: string; stacks: Stack[]; note?: string }) {
  const find = createMemo(() => finder(search()));
  const rows = createMemo(() => props.stacks.filter((s) => matches(find(), s.name)));
  return (
    <>
      <div class="toolbar">
        <Search label={`Search ${props.label.toLowerCase()}`} />
        <Show when={props.note}>
          <span class="muted small">{props.note}</span>
        </Show>
      </div>
      <Table label={props.label} head={["Item", "Qty"]} empty={rows().length === 0}>
        <For each={rows()}>
          {(s) => (
            <tr>
              <td>
                <Found text={s.name} find={find()} />
                <Show when={s.equipped}>
                  <span class="badge">equipped</span>
                </Show>
              </td>
              <td class="num">
                <Show
                  when={s.rank}
                  fallback={
                    <>
                      {number.format(s.qty)}
                      <Show when={s.max > 1}>
                        <span class="muted">/{number.format(s.max)}</span>
                      </Show>
                    </>
                  }
                >
                  {(rank) => (
                    <>
                      <span class="muted">{number.format(s.qty)} · </span>rank {rank()}
                    </>
                  )}
                </Show>
              </td>
            </tr>
          )}
        </For>
      </Table>
    </>
  );
}

function Drops(props: { drops: Stack[]; index: number; registered: Registered | null }) {
  const [chosen, setChosen] = createSignal<ReadonlySet<number>>(new Set());
  createEffect(on(() => props.index, () => setChosen(new Set<number>()), { defer: true }));
  createEffect(
    on(
      () => props.drops.map((d) => d.id),
      (ids) => {
        const live = new Set(ids);
        const kept = [...chosen()].filter((id) => live.has(id));
        if (kept.length < chosen().size) setChosen(new Set(kept));
      },
    ),
  );
  const find = createMemo(() => finder(search()));
  const rows = createMemo(() => props.drops.filter((s) => matches(find(), s.name)));
  const toggle = (id: number) => {
    const next = new Set(chosen());
    if (!next.delete(id)) next.add(id);
    setChosen(next);
  };
  const accept = async (ids: number[]) => {
    try {
      report(await api.act(targetsOf([props.index]), { pick_up: ids }), `accepting ${ids.length} drop${ids.length === 1 ? "" : "s"}`);
      setChosen(new Set<number>());
    } catch (e) {
      report([{ account: props.index, error: e instanceof Error ? e.message : String(e) }], "");
    }
  };
  return (
    <>
      <div class="toolbar">
        <DropSwitch pending={props.drops.length} registered={props.registered} />
        <Search label="Search drops" />
        <button type="button" disabled={chosen().size === 0} onClick={() => accept([...chosen()])}>
          Accept {chosen().size || ""}
        </button>
        <button type="button" disabled={props.drops.length === 0} onClick={() => accept(props.drops.map((d) => d.id))}>
          Accept all
        </button>
      </div>
      <Table
        label="Drops"
        head={[
          <label class="check">
            <input
              type="checkbox"
              aria-label="Select every drop"
              checked={rows().length > 0 && rows().every((d) => chosen().has(d.id))}
              onChange={(e) => setChosen(new Set(e.currentTarget.checked ? rows().map((d) => d.id) : []))}
            />
            Item
          </label>,
          "Qty",
        ]}
        empty={rows().length === 0}
      >
        <For each={rows()}>
          {(s) => (
            <tr>
              <td>
                <label class="check">
                  <input type="checkbox" checked={chosen().has(s.id)} onChange={() => toggle(s.id)} />
                  <Found text={s.name} find={find()} />
                </label>
              </td>
              <td class="num">{number.format(s.qty)}</td>
            </tr>
          )}
        </For>
      </Table>
    </>
  );
}

function parseItem(text: string): Item | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  return /^\d+$/.test(trimmed) ? Number(trimmed) : trimmed;
}

function knownItem(game: Game, id: number): string | undefined {
  return [...game.drops, ...game.inventory, ...game.temp].find((s) => s.id === id)?.name;
}

function itemName(game: Game, item: Item): string {
  if (typeof item === "string") return item;
  return knownItem(game, item) ?? `item ID ${item}`;
}

function knownQuest(game: Game, id: number): string | undefined {
  return game.quests.find((q) => q.id === id)?.name;
}

function questName(game: Game, id: number): string {
  return knownQuest(game, id) ?? `quest ID ${id}`;
}

const ITEM_UNKNOWN = "Registered by item ID. Its name shows once it's in the account's drops, inventory or temp items";
const QUEST_UNKNOWN = "Its name shows while the account has the quest accepted";

const [dropList, setDropList] = createSignal<"pending" | "registered">("pending");
const [questList, setQuestList] = createSignal<"accepted" | "registered">("accepted");

function ListSwitch<T extends string>(props: {
  name: string;
  label: string;
  value: T;
  choices: [T, string, number][];
  onChange: (value: T) => void;
}) {
  return (
    <div class="segments" role="radiogroup" aria-label={props.label}>
      <For each={props.choices}>
        {([value, label, count]) => (
          <label class="segment">
            <input
              type="radio"
              name={props.name}
              value={value}
              checked={props.value === value}
              onChange={() => props.onChange(value)}
            />
            {label}
            <span class="count">{count}</span>
          </label>
        )}
      </For>
    </div>
  );
}

function DropSwitch(props: { pending: number; registered: Registered | null }) {
  return (
    <Show when={props.registered}>
      {(registered) => (
        <ListSwitch
          name="drop-list"
          label="Drops to show"
          value={dropList()}
          choices={[
            ["pending", "Pending", props.pending],
            ["registered", "Registered", registered().drops.length],
          ]}
          onChange={setDropList}
        />
      )}
    </Show>
  );
}

function QuestSwitch(props: { accepted: number; registered: Registered | null }) {
  return (
    <Show when={props.registered}>
      {(registered) => (
        <ListSwitch
          name="quest-list"
          label="Quests to show"
          value={questList()}
          choices={[
            ["accepted", "Accepted", props.accepted],
            ["registered", "Registered", registered().quests.length],
          ]}
          onChange={setQuestList}
        />
      )}
    </Show>
  );
}

function Unnamed(props: { tip: string }) {
  return (
    <span class="muted" data-tip={props.tip}>
      Unknown name
    </span>
  );
}

function ItemLabel(props: { game: Game; item: Item }) {
  const item = props.item;
  if (typeof item === "string") return <>{item}</>;
  return (
    <Show when={knownItem(props.game, item)} fallback={<span data-tip={ITEM_UNKNOWN}><span class="muted">ID</span> {item}</span>}>
      {(name) => (
        <>
          {name()} <span class="muted">#{item}</span>
        </>
      )}
    </Show>
  );
}

function Unregister(props: { name: string; onClick: () => void }) {
  return (
    <button type="button" class="icon-button" aria-label={`Unregister ${props.name}`} onClick={props.onClick}>
      <Icon name="close" />
    </button>
  );
}

const removeHead = () => <span class="visually-hidden">Unregister</span>;

function RegisteredDrops(props: { game: Game; registered: Registered; index: number }) {
  const [text, setText] = createSignal("");
  const find = createMemo(() => finder(search()));
  const nameOf = (item: Item) => (typeof item === "string" ? item : knownItem(props.game, item));
  const rows = createMemo(() => props.registered.drops.filter((item) => matches(find(), `${nameOf(item) ?? ""} ${item}`)));
  const register = (e: SubmitEvent) => {
    e.preventDefault();
    const item = parseItem(text());
    if (item === null) return;
    void act({ registry: { register_drop: item } }, `registered ${itemName(props.game, item)}`, [props.index]);
    setText("");
  };
  return (
    <>
      <div class="toolbar">
        <DropSwitch pending={props.game.drops.length} registered={props.registered} />
        <form class="register" onSubmit={register}>
          <input
            type="text"
            aria-label="Drop to register, by name or item ID"
            placeholder="Name or item ID"
            value={text()}
            onInput={(e) => setText(e.currentTarget.value)}
          />
          <button type="submit" disabled={parseItem(text()) === null}>
            Register
          </button>
        </form>
        <Search label="Search registered drops" />
      </div>
      <Table label="Registered drops" head={["Item", "ID", removeHead()]} empty={rows().length === 0}>
        <For each={rows()}>
          {(item) => (
            <tr>
              <td>
                <Show when={nameOf(item)} fallback={<Unnamed tip={ITEM_UNKNOWN} />}>
                  {(name) => <Found text={name()} find={find()} />}
                </Show>
              </td>
              <td class="num">{typeof item === "number" ? <Found text={String(item)} find={find()} /> : ""}</td>
              <td class="num">
                <Unregister
                  name={itemName(props.game, item)}
                  onClick={() => act({ registry: { unregister_drop: item } }, `unregistered ${itemName(props.game, item)}`, [props.index])}
                />
              </td>
            </tr>
          )}
        </For>
      </Table>
    </>
  );
}

function RegisteredQuests(props: { game: Game; registered: Registered; index: number }) {
  const [quest, setQuest] = createSignal("");
  const [reward, setReward] = createSignal("");
  const id = () => (/^\d+$/.test(quest().trim()) ? Number(quest().trim()) : null);
  const rewardId = () => (/^\d+$/.test(reward().trim()) ? Number(reward().trim()) : null);
  const rewardValid = () => !reward().trim() || rewardId() !== null;
  const find = createMemo(() => finder(search()));
  const rows = createMemo(() =>
    props.registered.quests.map((q) => q.id).filter((questId) => matches(find(), `${knownQuest(props.game, questId) ?? ""} ${questId}`)),
  );
  const rewardOf = (questId: number) => props.registered.quests.find((q) => q.id === questId)?.reward;
  const register = (e: SubmitEvent) => {
    e.preventDefault();
    const questId = id();
    if (questId === null || !rewardValid()) return;
    const chosen = rewardId();
    const registration = chosen === null ? { id: questId } : { id: questId, reward: chosen };
    void act({ registry: { register_quest: registration } }, `registered ${questName(props.game, questId)}`, [props.index]);
    setQuest("");
    setReward("");
  };
  return (
    <>
      <div class="toolbar">
        <QuestSwitch accepted={props.game.quests.length} registered={props.registered} />
        <form class="register" onSubmit={register}>
          <input
            type="text"
            inputmode="numeric"
            class="short"
            aria-label="Quest ID to register"
            placeholder="Quest ID"
            aria-invalid={quest().trim() !== "" && id() === null}
            value={quest()}
            onInput={(e) => setQuest(e.currentTarget.value)}
          />
          <input
            type="text"
            inputmode="numeric"
            class="short"
            aria-label="Reward item ID, optional"
            placeholder="Reward ID"
            aria-invalid={!rewardValid()}
            value={reward()}
            onInput={(e) => setReward(e.currentTarget.value)}
          />
          <button type="submit" disabled={id() === null || !rewardValid()}>
            Register
          </button>
        </form>
        <Search label="Search registered quests" />
      </div>
      <Table label="Registered quests" head={["Quest", "ID", "Reward", removeHead()]} empty={rows().length === 0}>
        <For each={rows()}>
          {(questId) => (
            <tr>
              <td>
                <Show when={knownQuest(props.game, questId)} fallback={<Unnamed tip={QUEST_UNKNOWN} />}>
                  {(name) => <Found text={name()} find={find()} />}
                </Show>
              </td>
              <td class="num">
                <Found text={String(questId)} find={find()} />
              </td>
              <td class="num">
                <Show when={rewardOf(questId)} keyed>
                  {(reward) => <ItemLabel game={props.game} item={reward} />}
                </Show>
              </td>
              <td class="num">
                <Unregister
                  name={questName(props.game, questId)}
                  onClick={() =>
                    act({ registry: { unregister_quest: questId } }, `unregistered ${questName(props.game, questId)}`, [props.index])
                  }
                />
              </td>
            </tr>
          )}
        </For>
      </Table>
    </>
  );
}

function Lists(props: { game: Game; view: View; index: number; registered: Registered | null }) {
  const g = () => props.game;
  const find = createMemo(() => finder(search()));
  return (
    <Switch>
      <Match when={props.view === "players"}>
        <div class="toolbar">
          <Search label="Search players" />
        </div>
        <Table label="Players" head={["Player", "Level"]} empty={!g().players.some((p) => matches(find(), hide(p.name)))}>
          <For each={g().players.filter((p) => matches(find(), hide(p.name)))}>
            {(p) => (
              <tr>
                <td>
                  <Found text={hide(p.name)} find={find()} />
                </td>
                <td class="num">{p.level ?? ""}</td>
              </tr>
            )}
          </For>
        </Table>
      </Match>
      <Match when={props.view === "drops" && dropList() === "registered" && props.registered}>
        {(registered) => <RegisteredDrops game={g()} registered={registered()} index={props.index} />}
      </Match>
      <Match when={props.view === "drops"}>
        <Drops drops={g().drops} index={props.index} registered={props.registered} />
      </Match>
      <Match when={props.view === "quests" && questList() === "registered" && props.registered}>
        {(registered) => <RegisteredQuests game={g()} registered={registered()} index={props.index} />}
      </Match>
      <Match when={props.view === "quests"}>
        <div class="toolbar">
          <QuestSwitch accepted={g().quests.length} registered={props.registered} />
          <Search label="Search quests" />
        </div>
        <Table label="Quests" head={["Quest", "Progress"]} empty={!g().quests.some((q) => matches(find(), q.name))}>
          <For each={g().quests.filter((q) => matches(find(), q.name))}>
            {(q) => (
              <>
                <tr>
                  <td>
                    <Found text={q.name} find={find()} /> <span class="muted">#{q.id}</span>
                  </td>
                  <td />
                </tr>
                <For each={q.items}>
                  {(item) => (
                    <tr class="turn-in">
                      <td>{item.name}</td>
                      <td class="num">
                        {number.format(item.have)}/{number.format(item.need)}
                      </td>
                    </tr>
                  )}
                </For>
              </>
            )}
          </For>
        </Table>
      </Match>
      <Match when={props.view === "temp"}>
        <Stacks label="Temporary items" stacks={g().temp} />
      </Match>
      <Match when={props.view === "inventory"}>
        <Stacks
          label="Inventory"
          stacks={g().inventory}
          note={`bag ${g().bag_used}/${g().bag_slots} · misc ${g().misc_used}`}
        />
      </Match>
      <Match when={props.view === "bank"}>
        <Show when={g().bank} fallback={<p class="muted empty">The bank shows once a script opens it</p>}>
          {(bank) => <Stacks label="Bank" stacks={bank()} note={`${g().bank_used}/${g().bank_slots}`} />}
        </Show>
      </Match>
      <Match when={props.view === "factions"}>
        <div class="toolbar">
          <Search label="Search factions" />
        </div>
        <Table label="Factions" head={["Faction", "Rank", "Rep"]} empty={!g().factions.some((f) => matches(find(), f.name))}>
          <For each={g().factions.filter((f) => matches(find(), f.name))}>
            {(f) => (
              <tr>
                <td>
                  <Found text={f.name} find={find()} />
                  <Show when={f.gained}>
                    <span class="badge">gained</span>
                  </Show>
                </td>
                <td class="num">{f.rank}</td>
                <td class="num">
                  {number.format(f.rep)}
                  <Show when={f.next !== null}>
                    <span class="muted">/{number.format(f.next ?? 0)}</span>
                  </Show>
                </td>
              </tr>
            )}
          </For>
        </Table>
      </Match>
    </Switch>
  );
}

const stillMotion = matchMedia("(prefers-reduced-motion: reduce)");
const DRIFT_MS = 150;
const FILL_FRAME_MS = 100;
const STILL_FRAME_MS = 250;

function Skill(props: { n: number; skill: Game["skills"][number] }) {
  let fill!: HTMLSpanElement;
  let label!: HTMLSpanElement;
  let end = 0;
  createEffect(() => {
    const { ready, left_ms } = props.skill;
    const next = performance.now() + left_ms;
    const animating = ready > 0 && ready < 1 && left_ms > 0;
    const frame = stillMotion.matches ? STILL_FRAME_MS : FILL_FRAME_MS;
    if (animating && fill.getAnimations().length > 0 && Math.abs(next - end) < DRIFT_MS) return;
    end = next;
    const gap = (1 - ready) * 100;
    for (const [el, sign] of [[fill, -1], [label, 1]] as const) {
      for (const running of el.getAnimations()) running.cancel();
      const from = `translateX(${sign * gap}%)`;
      el.style.transform = animating ? "" : from;
      if (animating) {
        el.animate([{ transform: from }, { transform: "none" }], {
          duration: left_ms,
          easing: `steps(${Math.max(1, Math.round(left_ms / frame))})`,
        });
      }
    }
  });
  return (
    <span class="skill">
      {props.n} {props.skill.name}
      <span class="fill" ref={fill} aria-hidden="true">
        <span ref={label}>
          {props.n} {props.skill.name}
        </span>
      </span>
      <Show when={props.skill.ready < 1}>
        <span class="visually-hidden"> (cooling down)</span>
      </Show>
    </span>
  );
}
