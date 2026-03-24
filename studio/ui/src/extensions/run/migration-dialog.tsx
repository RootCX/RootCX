import { useState, useEffect } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/button";
import { sendMessage } from "@/extensions/forge/store";
import type { SchemaChange } from "@/types";

let resolve: ((ok: boolean) => void) | null = null;
let setDialog: ((changes: SchemaChange[] | null) => void) | null = null;

export function showMigrationDialog(changes: SchemaChange[]): Promise<boolean> {
  return new Promise((res) => {
    resolve = res;
    setDialog?.(changes);
  });
}

function buildPrompt(changes: SchemaChange[]): string {
  const dangerous = changes.filter((c) => !c.safe);
  const lines = dangerous.map((c) => {
    const col = c.column ? ` "${c.column}"` : "";
    const detail = c.detail ? ` (${c.detail})` : "";
    return `- ${c.change_type} on "${c.entity}"${col}${detail}`;
  });
  return [
    "Schema drift detected. Generate exactly ONE migration SQL file for ALL the following changes:",
    "",
    ...lines,
    "",
    "Read `manifest.json` for the exact field types, required flags, enum_values, and defaults.",
    "Write the SQL to `backend/migrations/` with the next available number prefix (NNN_description.sql).",
    "Rules:",
    "- Handle existing data safely (use defaults for NOT NULL columns, backfill if needed)",
    "- Do NOT add CHECK constraints or column defaults unless manifest.json explicitly defines them",
    "- Do NOT touch the _migrations table (Core-managed)",
  ].join("\n");
}

export function MigrationDialogPortal() {
  const [changes, _setDialog] = useState<SchemaChange[] | null>(null);
  useEffect(() => { setDialog = _setDialog; return () => { setDialog = null; }; }, []);
  if (!changes) return null;
  return createPortal(
    <MigrationDialog changes={changes} onDone={(ok) => {
      _setDialog(null); resolve?.(ok); resolve = null;
    }} />,
    document.body,
  );
}

const CHANGE_META: Record<string, { icon: string; label: (c: SchemaChange) => string }> = {
  create_table:  { icon: "+", label: (c) => `Create table "${c.entity}" (${c.detail ?? ""})` },
  drop_table:    { icon: "-", label: (c) => `Drop table "${c.entity}"` },
  add_column:    { icon: "+", label: (c) => `Add column "${c.column}" (${c.detail ?? "unknown"})` },
  drop_column:   { icon: "-", label: (c) => `Drop column "${c.column}"` },
  alter_type:    { icon: "~", label: (c) => `Change type of "${c.column}" (${c.detail ?? ""})` },
  set_not_null:  { icon: "~", label: (c) => `Set "${c.column}" to NOT NULL` },
  drop_not_null: { icon: "~", label: (c) => `Drop NOT NULL on "${c.column}"` },
  set_default:   { icon: "~", label: (c) => `Set default on "${c.column}" = ${c.detail ?? ""}` },
  drop_default:  { icon: "-", label: (c) => `Drop default on "${c.column}"` },
  update_enum:   { icon: "~", label: (c) => `Update enum on "${c.column}" → [${c.detail ?? ""}]` },
  drop_enum:     { icon: "-", label: (c) => `Drop enum constraint on "${c.column}"` },
};

const DEFAULT_META = { icon: "~", label: (c: SchemaChange) => `${c.change_type} on "${c.column}"` };

function ChangeRow({ c }: { c: SchemaChange }) {
  const m = CHANGE_META[c.change_type] ?? DEFAULT_META;
  const color = c.safe ? "text-green-500" : "text-red-400";
  const badge = c.safe ? "auto" : "migration";
  return (
    <div className="pl-3 flex items-center gap-1.5">
      <span className={color}>{m.icon}</span>
      <span className="flex-1">{m.label(c)}</span>
      <span className={`text-[10px] px-1.5 rounded ${c.safe ? "bg-green-500/10 text-green-400" : "bg-red-500/10 text-red-400"}`}>
        {badge}
      </span>
    </div>
  );
}

function MigrationDialog({ changes, onDone }: { changes: SchemaChange[]; onDone: (ok: boolean) => void }) {
  const hasDangerous = changes.some((c) => !c.safe);
  const grouped = new Map<string, SchemaChange[]>();
  for (const c of changes) {
    let list = grouped.get(c.entity);
    if (!list) grouped.set(c.entity, list = []);
    list.push(c);
  }

  const title = hasDangerous ? "Schema Changes Detected" : "Schema Changes";

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[20vh] animate-portal-overlay-in" onClick={() => onDone(false)}>
      <div className="absolute inset-0 bg-black/60 backdrop-blur-[6px]" />
      <div
        className="relative w-full max-w-md rounded-xl border border-white/[0.06] bg-card shadow-dialog animate-portal-content-in"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onDone(false)}
      >
        <div className="px-5 py-4">
          <div className="text-[13px] font-semibold tracking-[-0.01em] mb-3">{title}</div>
          <div className="space-y-3 max-h-60 overflow-y-auto text-xs font-mono">
            {[...grouped.entries()].map(([entity, items]) => (
              <div key={entity}>
                <div className="text-muted-foreground mb-1">Table: {entity}</div>
                {items.map((c, i) => <ChangeRow key={i} c={c} />)}
              </div>
            ))}
          </div>
          <div className="flex justify-end gap-2 mt-5 pt-4 border-t border-white/[0.04]">
            <Button variant="outline" size="sm" onClick={() => onDone(false)}>Cancel</Button>
            {hasDangerous && (
              <Button variant="outline" size="sm" onClick={() => { sendMessage(buildPrompt(changes)); onDone(false); }}>
                Generate with AI
              </Button>
            )}
            <Button size="sm" onClick={() => onDone(true)}>
              {hasDangerous ? "Continue anyway" : "Apply"}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
