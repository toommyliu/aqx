import { createEffect, createSignal, onCleanup } from "solid-js";

type Timing = { progress: number; leftMs: number; running: boolean; sampledAt: number };
type Entry = { visible: boolean; next: number; update: (now: number) => number };

const entries = new Map<Element, Entry>();
const stillMotion = matchMedia("(prefers-reduced-motion: reduce)");
let observer: IntersectionObserver | undefined;
let timeout: ReturnType<typeof setTimeout> | undefined;

function flush() {
  clearTimeout(timeout);
  timeout = undefined;
  if (document.visibilityState !== "visible") return;
  const now = performance.now();
  let next = Infinity;
  for (const entry of entries.values()) {
    if (!entry.visible) continue;
    if (entry.next <= now) entry.next = entry.update(now);
    next = Math.min(next, entry.next);
  }
  if (next < Infinity) {
    const step = stillMotion.matches ? 1000 : 250;
    timeout = setTimeout(flush, Math.max(1, Math.ceil(next / step) * step - performance.now()));
  }
}

function visibilityChanged() {
  for (const entry of entries.values()) entry.next = 0;
  flush();
}

export function createCountdown(timing: () => Timing) {
  let snapshot = timing();
  const [percent, setPercent] = createSignal(Math.floor(snapshot.progress * 100));
  const [seconds, setSeconds] = createSignal(Math.ceil(snapshot.leftMs / 1000));
  let refresh: (() => void) | undefined;

  const update = (now: number) => {
    const elapsed = snapshot.running ? Math.max(0, now - snapshot.sampledAt) : 0;
    const left = Math.max(0, snapshot.leftMs - elapsed);
    const rate = snapshot.running && snapshot.leftMs > 0 ? (1 - snapshot.progress) / snapshot.leftMs : 0;
    const filled = Math.min(100, Math.floor((snapshot.progress + rate * elapsed) * 100));
    const remaining = Math.ceil(left / 1000);
    setPercent(filled);
    setSeconds(remaining);
    if (rate <= 0 || left <= 0) return Infinity;
    const nextFill = snapshot.sampledAt + ((filled + 1) / 100 - snapshot.progress) / rate;
    const nextSecond = snapshot.sampledAt + snapshot.leftMs - (remaining - 1) * 1000;
    return Math.min(nextFill, nextSecond);
  };

  createEffect(() => {
    snapshot = timing();
    if (refresh) {
      refresh();
    } else {
      update(performance.now());
    }
  });

  const observe = (element: HTMLSpanElement) => {
    refresh = observeTime(element, update);
  };
  return { percent, seconds, observe };
}

export function observeTime(element: Element, update: (now: number) => number) {
  if (!observer) {
    observer = new IntersectionObserver((changes) => {
      for (const change of changes) {
        const entry = entries.get(change.target);
        if (entry) {
          entry.visible = change.isIntersecting;
          entry.next = 0;
        }
      }
      flush();
    });
    document.addEventListener("visibilitychange", visibilityChanged);
  }
  const entry = { visible: false, next: 0, update };
  entries.set(element, entry);
  observer.observe(element);
  onCleanup(() => {
    entries.delete(element);
    observer?.unobserve(element);
    if (entries.size === 0) {
      observer?.disconnect();
      observer = undefined;
      document.removeEventListener("visibilitychange", visibilityChanged);
    }
    flush();
  });
  return () => {
    entry.next = 0;
    flush();
  };
}
