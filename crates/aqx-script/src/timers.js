(alarm) => {
  const timers = new Map();
  let next = 1;
  const arm = () => {
    let due = Infinity;
    for (const t of timers.values()) if (t.due < due) due = t.due;
    alarm(due === Infinity ? -1 : Math.max(0, due - Date.now()));
  };
  const add = (fn, ms, args, repeat) => {
    if (typeof fn !== "function") throw new TypeError("the callback must be a function");
    const delay = Math.max(0, Number(ms) || 0);
    const id = next++;
    timers.set(id, { due: Date.now() + delay, fn, args, every: repeat ? Math.max(1, delay) : null });
    arm();
    return id;
  };
  const clear = (id) => {
    if (timers.delete(id)) arm();
  };
  const run = () => {
    const now = Date.now();
    const due = [...timers].filter(([, t]) => t.due <= now).sort(([, a], [, b]) => a.due - b.due);
    try {
      for (const [id, t] of due) {
        if (timers.get(id) !== t) continue;
        if (t.every === null) {
          timers.delete(id);
        } else {
          t.due += t.every;
          if (t.due <= now) t.due = now + t.every;
        }
        t.fn(...t.args);
      }
    } finally {
      arm();
    }
  };
  return {
    setTimeout: (fn, ms, ...args) => add(fn, ms, args, false),
    setInterval: (fn, ms, ...args) => add(fn, ms, args, true),
    clearTimeout: clear,
    clearInterval: clear,
    run,
  };
}
