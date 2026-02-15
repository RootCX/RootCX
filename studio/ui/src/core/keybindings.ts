import type { Disposable } from "./registry";

const isMac = navigator.platform.startsWith("Mac");

interface Binding {
  commandId: string;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  key: string;
}

type ModKey = "ctrl" | "alt" | "shift" | "meta";
const MAC_SYMBOLS: [ModKey, string][] = [["ctrl", "\u2303"], ["alt", "\u2325"], ["shift", "\u21E7"], ["meta", "\u2318"]];
const PC_LABELS: [ModKey, string][] = [["ctrl", "Ctrl"], ["alt", "Alt"], ["shift", "Shift"], ["meta", "Meta"]];

const bindings: Binding[] = [];
let installed = false;

function parse(kb: string): Omit<Binding, "commandId"> {
  const mods = { ctrl: false, shift: false, alt: false, meta: false };
  let key = "";
  for (const p of kb.toLowerCase().split("+")) {
    if (p === "mod") mods[isMac ? "meta" : "ctrl"] = true;
    else if (p in mods) (mods as Record<string, boolean>)[p] = true;
    else key = p;
  }
  return { ...mods, key };
}

function matches(b: Binding, e: KeyboardEvent): boolean {
  return (
    b.key === e.key.toLowerCase() &&
    b.ctrl === e.ctrlKey &&
    b.shift === e.shiftKey &&
    b.alt === e.altKey &&
    b.meta === e.metaKey
  );
}

function format(b: Binding): string {
  const table = isMac ? MAC_SYMBOLS : PC_LABELS;
  const parts = table.filter(([k]) => b[k]).map(([, v]) => v);
  parts.push(b.key.length === 1 ? b.key.toUpperCase() : b.key[0].toUpperCase() + b.key.slice(1));
  return parts.join(isMac ? "" : "+");
}

export function registerKeybinding(commandId: string, keybinding: string): Disposable {
  const binding: Binding = { commandId, ...parse(keybinding) };
  bindings.push(binding);
  return {
    dispose: () => {
      const idx = bindings.indexOf(binding);
      if (idx >= 0) bindings.splice(idx, 1);
    },
  };
}

export function getKeybindingForCommand(commandId: string): string | undefined {
  const b = bindings.find((x) => x.commandId === commandId);
  return b ? format(b) : undefined;
}

export function installGlobalListener(executor: (commandId: string) => void) {
  if (installed) return;
  installed = true;
  document.addEventListener("keydown", (e) => {
    if (e.defaultPrevented) return;
    for (const b of bindings) {
      if (matches(b, e)) {
        e.preventDefault();
        e.stopPropagation();
        executor(b.commandId);
        return;
      }
    }
  });
}
