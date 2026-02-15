type Listener = () => void;

let isOpen = false;
const listeners = new Set<Listener>();

function notify() {
  listeners.forEach((fn) => fn());
}

export function openPalette() {
  if (!isOpen) {
    isOpen = true;
    notify();
  }
}

export function closePalette() {
  if (isOpen) {
    isOpen = false;
    notify();
  }
}

export function getIsOpen() {
  return isOpen;
}

export function subscribe(fn: Listener) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}
