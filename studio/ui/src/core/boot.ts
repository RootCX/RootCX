import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { initAuth } from "./auth";

interface BootState {
  ready: boolean;
  status: string;
}

let state: BootState = { ready: false, status: "Starting up…" };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach((fn) => fn()); }

const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
const getSnapshot = () => snapshot;
export function useBoot() { return useSyncExternalStore(subscribe, getSnapshot); }

function resolve() {
  state = { ready: true, status: "" };
  emit();
  initAuth();
}

function reject(err: unknown) {
  state = { ready: true, status: `Boot failed: ${err}` };
  emit();
}

export function startBoot() {
  listen<string>("boot-progress", (e) => {
    state = { ...state, status: e.payload };
    emit();
  });
  invoke("await_boot").then(resolve, reject);
}
