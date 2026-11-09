import { defineAppHotkeys } from "@tanstack/solid-hotkeys";

const account = "Accounts";
const app = "App";

export const appHotkeys = defineAppHotkeys({
  "account.login": { hotkey: "I", meta: { name: "Log in, then run the script", group: account } },
  "account.run": { hotkey: "O", meta: { name: "Pick a script and run it", group: account } },
  "account.stop": { hotkey: "S", meta: { name: "Stop the script", group: account } },
  "account.pause": { hotkey: "P", meta: { name: "Pause or resume the script", group: account } },
  "account.restart": { hotkey: null, meta: { name: "Restart the script", group: account } },
  "account.server": { hotkey: null, meta: { name: "Switch server", group: account } },
  "account.logout": { hotkey: null, meta: { name: "Log out", group: account } },
  "account.next": { hotkey: "J", meta: { name: "Next account", group: account } },
  "account.previous": { hotkey: "K", meta: { name: "Previous account", group: account } },
  "account.mark": { hotkey: "X", meta: { name: "Tick or untick the account", group: account } },
  "account.markAll": { hotkey: "Mod+A", meta: { name: "Select every account, or none", group: account } },
  "search.focus": { hotkey: "/", meta: { name: "Search the log or list", group: app } },
  "app.back": { hotkey: "Escape", meta: { name: "Clear the search, then leave select mode", group: app } },
  "app.keys": { hotkey: "?", meta: { name: "Keys", group: app } },
});

export const fixedKeys: ReadonlySet<string> = new Set(
  Object.entries(appHotkeys)
    .filter(([, key]) => key.meta?.group === app)
    .map(([id]) => id),
);

declare module "@tanstack/solid-hotkeys" {
  interface Register {
    appHotkeys: typeof appHotkeys;
  }
}
