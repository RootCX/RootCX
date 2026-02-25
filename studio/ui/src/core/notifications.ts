export interface Notification {
  id: string;
  message: string;
  type: "info" | "success" | "warning" | "error";
  action?: { label: string; run: () => void };
}

const items = new Map<string, Notification>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();
const listeners = new Set<() => void>();
let snapshot: Notification[] = [];

function emit() {
  snapshot = [...items.values()];
  listeners.forEach((fn) => fn());
}

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

export function notify(id: string, message: string, type: Notification["type"], action?: Notification["action"]) {
  clearTimeout(timers.get(id));
  items.set(id, { id, message, type, action });
  emit();
  if (type === "info" || type === "success") timers.set(id, setTimeout(() => dismiss(id), 5000));
}

export function dismiss(id: string) {
  clearTimeout(timers.get(id));
  timers.delete(id);
  if (items.delete(id)) emit();
}
