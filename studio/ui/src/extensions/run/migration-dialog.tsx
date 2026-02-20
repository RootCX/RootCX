import { useState, useEffect } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/button";
import type { SchemaChange } from "@/types";

let resolve: ((approved: boolean) => void) | null = null;
let setDialog: ((changes: SchemaChange[] | null) => void) | null = null;

export function showMigrationDialog(changes: SchemaChange[]): Promise<boolean> {
  return new Promise((res) => {
    resolve = res;
    setDialog?.(changes);
  });
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

const CHANGE_META: Record<string, { icon: string; color: string; label: (c: SchemaChange) => string }> = {
  create_table:  { icon: "+", color: "text-green-500",  label: (c) => `Create table "${c.entity}" (${c.detail ?? ""})` },
  drop_table:    { icon: "-", color: "text-red-500",    label: (c) => `Drop table "${c.entity}"` },
  add_column:    { icon: "+", color: "text-green-500",  label: (c) => `Add column "${c.column}" (${c.detail ?? "unknown"})` },
  drop_column:   { icon: "-", color: "text-red-500",    label: (c) => `Drop column "${c.column}"` },
  alter_type:    { icon: "~", color: "text-yellow-500", label: (c) => `Change type of "${c.column}" (${c.detail ?? ""})` },
  set_not_null:  { icon: "~", color: "text-blue-500",   label: (c) => `Set "${c.column}" to NOT NULL` },
  drop_not_null: { icon: "~", color: "text-blue-500",   label: (c) => `Drop NOT NULL on "${c.column}"` },
  set_default:   { icon: "~", color: "text-blue-500",   label: (c) => `Set default on "${c.column}" = ${c.detail ?? ""}` },
  drop_default:  { icon: "-", color: "text-blue-500",   label: (c) => `Drop default on "${c.column}"` },
  update_enum:   { icon: "~", color: "text-blue-500",   label: (c) => `Update enum on "${c.column}" → [${c.detail ?? ""}]` },
  drop_enum:     { icon: "-", color: "text-red-500",    label: (c) => `Drop enum constraint on "${c.column}"` },
};

const DEFAULT_META = { icon: "~", color: "text-blue-500", label: (c: SchemaChange) => `${c.change_type} on "${c.column}"` };

function MigrationDialog({ changes, onDone }: { changes: SchemaChange[]; onDone: (ok: boolean) => void }) {
  const grouped = new Map<string, SchemaChange[]>();
  for (const c of changes) {
    let list = grouped.get(c.entity);
    if (!list) grouped.set(c.entity, list = []);
    list.push(c);
  }

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[20vh]" onClick={() => onDone(false)}>
      <div className="absolute inset-0 bg-black/50" />
      <div
        className="relative w-full max-w-md rounded-lg border border-border bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onDone(false)}
      >
        <div className="px-4 py-3">
          <div className="text-sm font-medium mb-3">Schema Migration Required</div>
          <div className="space-y-3 max-h-60 overflow-y-auto text-xs font-mono">
            {[...grouped.entries()].map(([entity, items]) => (
              <div key={entity}>
                <div className="text-muted-foreground mb-1">Table: {entity}</div>
                {items.map((c, i) => {
                  const m = CHANGE_META[c.change_type] ?? DEFAULT_META;
                  return (
                    <div key={i} className="pl-3">
                      <span className={m.color}>{m.icon}</span> {m.label(c)}
                    </div>
                  );
                })}
              </div>
            ))}
          </div>
          <div className="flex justify-end gap-2 mt-4">
            <Button variant="outline" size="sm" onClick={() => onDone(false)}>Cancel</Button>
            <Button size="sm" onClick={() => onDone(true)}>Apply Migration</Button>
          </div>
        </div>
      </div>
    </div>
  );
}
