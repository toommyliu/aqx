import { HotkeysProvider } from "@tanstack/solid-hotkeys";
import { QueryClient, QueryClientProvider, createMutation, createQuery } from "@tanstack/solid-query";
import { render } from "solid-js/web";
import { App, applyTheme } from "./App";
import { appHotkeys, fixedKeys } from "./hotkeys";
import { fail } from "./store";
import { api } from "./wire";
import "./styles.css";

const client = new QueryClient({
  defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } },
});

function Root() {
  const saved = createQuery(() => ({ queryKey: ["keys"], queryFn: api.keys, staleTime: Infinity }));
  const save = createMutation(() => ({
    mutationFn: api.saveKeys,
    onError: (e: Error) => fail(`Couldn't save the keys: ${e.message}`),
  }));
  return (
    <HotkeysProvider
      appHotkeys={appHotkeys}
      customHotkeys={saved.data && Object.fromEntries(Object.entries(saved.data).filter(([id]) => !fixedKeys.has(id)))}
      onCustomHotkeysChange={(custom) => save.mutate(custom)}
      defaultOptions={{ hotkey: { ignoreInputs: true } }}
    >
      <App />
    </HotkeysProvider>
  );
}

applyTheme();
const root = document.getElementById("root");
if (root)
  render(
    () => (
      <QueryClientProvider client={client}>
        <Root />
      </QueryClientProvider>
    ),
    root,
  );
