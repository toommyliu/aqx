import { autoUpdate, computePosition, flip, offset, shift } from "@floating-ui/dom";
import { createVirtualizer } from "@tanstack/solid-virtual";
import ArrowDown from "lucide-solid/icons/arrow-down";
import ArrowUp from "lucide-solid/icons/arrow-up";
import ChevronDown from "lucide-solid/icons/chevron-down";
import ChevronLeft from "lucide-solid/icons/chevron-left";
import Ellipsis from "lucide-solid/icons/ellipsis";
import Eye from "lucide-solid/icons/eye";
import EyeOff from "lucide-solid/icons/eye-off";
import File from "lucide-solid/icons/file";
import FileCog from "lucide-solid/icons/file-cog";
import Folder from "lucide-solid/icons/folder";
import Keyboard from "lucide-solid/icons/keyboard";
import LogIn from "lucide-solid/icons/log-in";
import LogOut from "lucide-solid/icons/log-out";
import Moon from "lucide-solid/icons/moon";
import PanelLeftClose from "lucide-solid/icons/panel-left-close";
import PanelLeftOpen from "lucide-solid/icons/panel-left-open";
import Pause from "lucide-solid/icons/pause";
import Plus from "lucide-solid/icons/plus";
import Play from "lucide-solid/icons/play";
import Power from "lucide-solid/icons/power";
import RotateCcw from "lucide-solid/icons/rotate-ccw";
import Server from "lucide-solid/icons/server";
import SlidersHorizontal from "lucide-solid/icons/sliders-horizontal";
import Square from "lucide-solid/icons/square";
import Sun from "lucide-solid/icons/sun";
import SunMoon from "lucide-solid/icons/sun-moon";
import Users from "lucide-solid/icons/users";
import X from "lucide-solid/icons/x";
import { type DisplayHotkeyChord, formatForDisplay } from "@tanstack/solid-hotkeys";
import { createQuery } from "@tanstack/solid-query";
import { For, Index, type JSX, Show, createSignal, createUniqueId, onMount } from "solid-js";
import { Dynamic } from "solid-js/web";
import { api } from "./wire";

const ICONS = {
  login: LogIn,
  logout: LogOut,
  play: Play,
  stop: Square,
  pause: Pause,
  restart: RotateCcw,
  server: Server,
  users: Users,
  config: FileCog,
  close: X,
  back: ChevronLeft,
  expand: ChevronDown,
  up: ArrowUp,
  folder: Folder,
  file: File,
  options: SlidersHorizontal,
  down: ArrowDown,
  keyboard: Keyboard,
  hideList: PanelLeftClose,
  showList: PanelLeftOpen,
  power: Power,
  sun: Sun,
  moon: Moon,
  auto: SunMoon,
  more: Ellipsis,
  add: Plus,
  show: Eye,
  hide: EyeOff,
} as const;

export type IconName = keyof typeof ICONS;

export function Icon(props: { name: IconName }) {
  return <Dynamic component={ICONS[props.name]} class="icon" size={14} stroke-width={2} aria-hidden="true" />;
}

export function KeyCaps(props: { hotkey: string; custom?: boolean }) {
  const chord = () => props.hotkey as DisplayHotkeyChord;
  return (
    <span class="keycaps" classList={{ custom: props.custom }} aria-label={formatForDisplay(chord())}>
      <For each={formatForDisplay(chord(), { parts: true })}>{(part) => <kbd aria-hidden="true">{part}</kbd>}</For>
    </span>
  );
}

export function Modal(props: {
  title: string;
  onClose: () => void;
  onCancel?: (e: Event) => void;
  canClose?: () => boolean;
  lightDismiss?: boolean;
  wide?: boolean;
  tall?: boolean;
  children: JSX.Element;
  footer?: JSX.Element;
}) {
  let ref!: HTMLDialogElement;
  const heading = createUniqueId();
  onMount(() => ref.showModal());
  const tryClose = () => {
    if (props.canClose?.() !== false) ref.close();
  };
  return (
    <dialog
      ref={ref}
      classList={{ wide: props.wide, tall: props.tall }}
      aria-labelledby={heading}
      onClose={() => {
        if (props.canClose?.() === false) ref.showModal();
        else props.onClose();
      }}
      onCancel={(e) => {
        props.onCancel?.(e);
        if (!e.defaultPrevented && props.canClose?.() === false) e.preventDefault();
      }}
      onClick={(e) => {
        if (e.target === ref && props.lightDismiss !== false) tryClose();
      }}
    >
      <div class="dialog-head">
        <h2 id={heading}>{props.title}</h2>
        <button type="button" class="icon-button" aria-label="Close" onClick={tryClose}>
          <Icon name="close" />
        </button>
      </div>
      <div class="dialog-body">{props.children}</div>
      <Show when={props.footer}>
        <div class="dialog-foot">{props.footer}</div>
      </Show>
    </dialog>
  );
}

export function arrows(e: KeyboardEvent, container: HTMLElement, selector = "[data-nav]") {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
  const items = [...container.querySelectorAll<HTMLElement>(selector)];
  if (items.length === 0) return;
  const at = items.findIndex((item) => item === document.activeElement);
  const next =
    e.key === "Home"
      ? 0
      : e.key === "End"
        ? items.length - 1
        : at === -1
          ? 0
          : Math.min(items.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)));
  e.preventDefault();
  items[next]?.focus();
}

export function typing(target: EventTarget | null) {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

export function clock(ms: number) {
  const d = new Date(ms);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

export function span(secs: number) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${secs}s`;
}

export const canHover = () => matchMedia("(hover: hover)").matches;

export function Found(props: { text: string; find: RegExp | null }) {
  const parts = () => (props.find ? props.text.split(new RegExp(`(${props.find.source})`, "giu")) : [props.text]);
  return <Index each={parts()}>{(part, i) => (i % 2 === 1 ? <mark>{part()}</mark> : <>{part()}</>)}</Index>;
}

export const number = new Intl.NumberFormat();

export const amount = (n: number, noun: string) => `${number.format(n)} ${noun}${n === 1 ? "" : "s"}`;

export function VirtualList<T>(props: {
  label: string;
  items: T[];
  row: (item: T, index: number) => JSX.Element;
  empty: string;
  ref?: (el: HTMLDivElement) => void;
}) {
  let scroller!: HTMLDivElement;
  const virtual = createVirtualizer({
    get count() {
      return props.items.length;
    },
    getScrollElement: () => scroller,
    estimateSize: () => (matchMedia("(max-width: 720px)").matches ? 46 : 28),
    overscan: 10,
  });
  const focusAt = (i: number) => {
    virtual.scrollToIndex(i);
    requestAnimationFrame(() => scroller.querySelector<HTMLElement>(`[data-index="${i}"] [data-nav]`)?.focus());
  };
  const move = (e: KeyboardEvent) => {
    const last = props.items.length - 1;
    const row = e.target instanceof HTMLElement ? e.target.closest<HTMLElement>("[data-index]") : null;
    const at = row ? Number(row.dataset.index) : -1;
    const page = Math.max(1, Math.floor(scroller.clientHeight / 28) - 1);
    const to: Record<string, number> = {
      ArrowDown: at + 1,
      ArrowUp: at - 1,
      PageDown: at + page,
      PageUp: at - page,
      Home: 0,
      End: last,
    };
    const next = to[e.key];
    if (next === undefined || last < 0) return;
    e.preventDefault();
    focusAt(Math.min(last, Math.max(0, next)));
  };
  return (
    <Show when={props.items.length > 0} fallback={<p class="muted empty">{props.empty}</p>}>
      <div
        class="vlist"
        ref={(el) => {
          scroller = el;
          props.ref?.(el);
        }}
        onKeyDown={move}
      >
        <ul aria-label={props.label} style={{ height: `${virtual.getTotalSize()}px` }}>
          <For each={virtual.getVirtualItems()}>
            {(v) => (
              <li
                data-index={v.index}
                ref={(el) => queueMicrotask(() => virtual.measureElement(el))}
                aria-setsize={props.items.length}
                aria-posinset={v.index + 1}
                style={{ transform: `translateY(${v.start}px)` }}
              >
                {props.row(props.items[v.index] as T, v.index)}
              </li>
            )}
          </For>
        </ul>
      </div>
    </Show>
  );
}

export function anchor(reference: Element, floating: HTMLElement, placement: "bottom-start" | "bottom-end" | "top" = "bottom-end") {
  floating.style.visibility = "hidden";
  return autoUpdate(reference, floating, () => {
    void computePosition(reference, floating, {
      placement,
      strategy: "fixed",
      middleware: [offset(6), flip(), shift({ padding: 8 })],
    }).then(({ x, y }) => {
      floating.style.left = `${x}px`;
      floating.style.top = `${y}px`;
      floating.style.visibility = "";
    });
  });
}

export function Tooltips() {
  let tip!: HTMLDivElement;
  let stop: (() => void) | undefined;
  const hide = () => {
    stop?.();
    stop = undefined;
    if (tip.matches(":popover-open")) tip.hidePopover();
  };
  const show = (target: EventTarget | null) => {
    const el = target instanceof Element ? target.closest<HTMLElement>("[data-tip]") : null;
    if (!el) return hide();
    tip.textContent = el.dataset.tip ?? "";
    if (!tip.matches(":popover-open")) tip.showPopover();
    stop?.();
    stop = anchor(el, tip, "top");
  };
  onMount(() => {
    const over = (e: Event) => (matchMedia("(hover: hover)").matches ? show(e.target) : undefined);
    const focus = (e: FocusEvent) =>
      e.target instanceof Element && e.target.matches(":focus-visible") ? show(e.target) : hide();
    const key = (e: KeyboardEvent) => (e.key === "Escape" ? hide() : undefined);
    document.addEventListener("pointerover", over);
    document.addEventListener("focusin", focus);
    document.addEventListener("focusout", hide);
    document.addEventListener("pointerdown", hide);
    document.addEventListener("keydown", key);
  });
  return <div class="tooltip" popover="manual" ref={tip} aria-hidden="true" />;
}

const [now, setNow] = createSignal(Date.now());
setInterval(() => setNow(Date.now()), 15_000);
const exact = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" });

export function Since(props: { at: number; prefix: string }) {
  const at = () => Math.round(props.at / 1000) * 1000;
  return (
    <time
      class="since"
      datetime={new Date(at()).toISOString()}
      tabindex="0"
      data-tip={exact.format(at())}
      aria-label={`${props.prefix} ${span(Math.max(0, Math.round((now() - at()) / 1000)))}, since ${exact.format(at())}`}
    >
      {props.prefix} {span(Math.max(0, Math.round((now() - at()) / 1000)))}
    </time>
  );
}

export const serverList = () => createQuery(() => ({ queryKey: ["servers"], queryFn: api.servers, staleTime: 60_000 }));

export function ServerName(props: { name: string }) {
  const servers = serverList();
  const canonical = () =>
    servers.data?.find((s) => s.name.toLowerCase() === props.name.toLowerCase())?.name ?? props.name;
  return <>{canonical()}</>;
}
