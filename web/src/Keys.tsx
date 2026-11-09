import {
  createAppHotkeyList,
  createFormatForDisplay,
  createHotkeyRecorder,
  useCustomHotkeys,
} from "@tanstack/solid-hotkeys";
import { For, Show, createMemo, createSignal } from "solid-js";
import { type AppHotkeyId, detectPlatform } from "@tanstack/hotkeys";
import { fixedKeys } from "./hotkeys";
import { setDialog } from "./store";
import { KeyCaps, Modal } from "./ui";

export function Keys() {
  const list = createAppHotkeyList();
  const custom = useCustomHotkeys();
  const format = createFormatForDisplay();
  const [editing, setEditing] = createSignal<AppHotkeyId | null>(null);
  const [error, setError] = createSignal("");
  const recorder = createHotkeyRecorder(() => ({
    recordBy: "key",
    onRecord: (hotkey) => {
      const id = editing();
      setEditing(null);
      if (!id) return;
      const conflicts = custom.setCustomHotkey(id, hotkey);
      setError(
        conflicts.length
          ? `${format(hotkey)} is already ${conflicts.map((c) => c.meta?.name ?? c.id).join(", ")}. Move that one first.`
          : "",
      );
    },
    onCancel: () => setEditing(null),
  }));
  const groups = createMemo(() => {
    const by = new Map<string, ReturnType<typeof list>>();
    for (const row of list()) {
      const group = row.appHotkey.meta?.group ?? "Other";
      by.set(group, [...(by.get(group) ?? []), row]);
    }
    return [...by];
  });
  const change = (id: AppHotkeyId) => {
    setError("");
    setEditing(id);
    recorder.startRecording();
  };
  return (
    <Modal
      title="Keys"
      wide
      onClose={() => setDialog(null)}
      onCancel={(e) => {
        if (recorder.isRecording()) e.preventDefault();
      }}
      footer={
        <>
          <p class="error grow" role="alert">
            {error()}
          </p>
          <button type="button" onClick={() => custom.resetCustomHotkeys()} disabled={Object.keys(custom.customHotkeys()).length === 0}>
            Reset all
          </button>
        </>
      }
    >
      <p class="muted small">
        Pick Change, then press the new key. Esc cancels. Keys you change show in blue and are saved for every browser. The App keys can't be changed.
      </p>
      <For each={groups()}>
        {([group, rows]) => (
          <table class="list keys">
            <caption>{group}</caption>
            <tbody>
              <For each={rows}>
                {(row) => {
                  const id = row.appHotkey.id as AppHotkeyId;
                  const fixed = fixedKeys.has(id);
                  const recording = () => editing() === id && recorder.isRecording();
                  return (
                    <tr>
                      <th scope="row">{row.appHotkey.meta?.name ?? id}</th>
                      <td class="key-cell">
                        <Show
                          when={!recording()}
                          fallback={
                            <span class="recording" aria-live="polite">
                              press a key…
                            </span>
                          }
                        >
                          <Show when={row.appHotkey.hotkey} fallback={<span class="muted">none</span>}>
                            {(hotkey) => (
                              <>
                                <Show when={typeof hotkey() === "string" ? String(hotkey()) : undefined} fallback={format(hotkey())}>
                                  {(chord) => <KeyCaps hotkey={chord()} custom={row.isCustom} />}
                                </Show>
                                <Show when={row.isCustom}>
                                  <span class="muted small"> changed</span>
                                </Show>
                              </>
                            )}
                          </Show>
                        </Show>
                      </td>
                      <td class="key-actions">
                        <Show when={!fixed}>
                        <button type="button" class="ghost" aria-label={`Change the key for ${row.appHotkey.meta?.name ?? id}`} onClick={() => change(id)}>
                          Change
                        </button>
                        <button
                          type="button"
                          class="ghost"
                          aria-label={`Clear the key for ${row.appHotkey.meta?.name ?? id}`}
                          disabled={!row.appHotkey.hotkey}
                          onClick={() => custom.setCustomHotkey(id, null)}
                        >
                          Clear
                        </button>
                        <button
                          type="button"
                          class="ghost"
                          aria-label={`Reset the key for ${row.appHotkey.meta?.name ?? id}`}
                          disabled={!row.isCustom}
                          onClick={() => custom.resetCustomHotkey(id)}
                        >
                          Reset
                        </button>
                        </Show>
                      </td>
                    </tr>
                  );
                }}
              </For>
            </tbody>
          </table>
        )}
      </For>
      <p class="muted small">
        {format("Mod+1")} to {format("Mod+9")} open the first nine accounts; hold {detectPlatform() === "mac" ? "⌘" : "Ctrl"} to see their numbers. In lists, ↑ ↓ move, Home and End jump, and Enter opens. Tabs move with ← →. In the script picker, ← goes up a
        folder. Esc closes a dialog.
      </p>
    </Modal>
  );
}
