import { invoke } from "@tauri-apps/api/core";

const BASE = "http://localhost:9100";

export interface ColumnInfo {
  column_name: string;
  data_type: string;
  is_nullable: boolean;
  column_default: string | null;
  ordinal_position: number;
}

export interface TableInfo {
  table_name: string;
  columns: ColumnInfo[];
  row_estimate: number;
}

export interface QueryResult {
  columns: string[];
  rows: unknown[][];
  row_count: number;
}

interface State {
  appId: string | null;
  tables: TableInfo[];
  loading: boolean;
  error: string | null;
  queryResult: QueryResult | null;
  queryError: string | null;
  queryLoading: boolean;
  queryElapsed: number | null;
}

let state: State = {
  appId: null, tables: [], loading: false, error: null,
  queryResult: null, queryError: null, queryLoading: false, queryElapsed: null,
};
const listeners = new Set<() => void>();
let snapshot = state;

function emit() {
  snapshot = { ...state };
  listeners.forEach((fn) => fn());
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

async function fetchTables(appId: string) {
  const res = await fetch(`${BASE}/api/v1/db/schemas/${encodeURIComponent(appId)}/tables`);
  if (!res.ok) throw new Error(await res.text().catch(() => "failed to fetch tables"));
  return (await res.json()) as TableInfo[];
}

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

export async function loadProject(projectPath: string) {
  state = { ...state, loading: true, error: null };
  emit();
  try {
    const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
    const { appId } = JSON.parse(raw) as { appId: string };
    state = { ...state, appId };
    emit();
    state = { ...state, tables: await fetchTables(appId), loading: false };
    emit();
  } catch (e) {
    state = { ...state, loading: false, error: errMsg(e) };
    emit();
  }
}

export async function refresh() {
  if (!state.appId) return;
  state = { ...state, loading: true, error: null };
  emit();
  try {
    state = { ...state, tables: await fetchTables(state.appId), loading: false };
    emit();
  } catch (e) {
    state = { ...state, loading: false, error: errMsg(e) };
    emit();
  }
}

export async function executeQuery(sql: string) {
  state = { ...state, queryLoading: true, queryError: null, queryElapsed: null };
  emit();
  const start = performance.now();
  try {
    const res = await fetch(`${BASE}/api/v1/db/query`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ sql, schema: state.appId }),
    });
    const elapsed = performance.now() - start;
    if (!res.ok) {
      const body = await res.json().catch(() => ({ error: "request failed" }));
      throw new Error(body.error ?? "request failed");
    }
    state = { ...state, queryResult: await res.json(), queryLoading: false, queryElapsed: elapsed };
    emit();
  } catch (e) {
    state = { ...state, queryLoading: false, queryError: errMsg(e), queryElapsed: performance.now() - start };
    emit();
  }
}

export function queryTable(table: string) {
  if (!state.appId) return;
  return executeQuery(`SELECT * FROM "${table}" LIMIT 200`);
}
