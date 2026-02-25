import { useState, useEffect, useSyncExternalStore } from "react";
import { ChevronRight, ChevronDown, Shield, Users, KeyRound, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { useProjectContext } from "@/components/layout/app-context";
import {
  subscribe, getSnapshot, loadProject, refresh, assignRole, revokeRole,
  type Role, type User, type Assignment, type Policy,
} from "./store";

const indent = (d: number) => ({ paddingLeft: `${d * 12 + 4}px` });
const CHEVRON = "h-3.5 w-3.5 shrink-0 text-muted-foreground";
const ROW = "flex w-full items-center gap-1 px-1 py-0.5 text-left text-xs hover:bg-accent";

function Section({ title, icon: Icon, count, children }: { title: string; icon: typeof Shield; count: number; children: React.ReactNode }) {
  const [open, setOpen] = useState(true);
  return (
    <div>
      <button onClick={() => setOpen((o) => !o)} className="flex w-full items-center gap-1.5 px-3 py-1 text-left text-[10px] font-semibold uppercase tracking-wider text-muted-foreground hover:bg-accent">
        {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
        <Icon className="h-3 w-3" />
        {title}
        <span className="ml-auto text-[9px] font-normal">{count}</span>
      </button>
      {open && children}
    </div>
  );
}

function RoleNode({ role, depth }: { role: Role; depth: number }) {
  const [expanded, setExpanded] = useState(false);
  const hasDetails = role.description || role.inherits.length > 0;
  return (
    <div>
      <button onClick={() => hasDetails && setExpanded((e) => !e)} className={ROW} style={indent(depth)}>
        {hasDetails
          ? (expanded ? <ChevronDown className={CHEVRON} /> : <ChevronRight className={CHEVRON} />)
          : <span className="w-3.5 shrink-0" />}
        <Shield className="h-3.5 w-3.5 shrink-0 text-amber-400" />
        <span className="truncate">{role.name}</span>
      </button>
      {expanded && (
        <div className="space-y-0.5 px-1 py-0.5 text-[10px] text-muted-foreground" style={indent(depth + 1)}>
          {role.description && <div>{role.description}</div>}
          {role.inherits.length > 0 && <div>Inherits: {role.inherits.join(", ")}</div>}
        </div>
      )}
    </div>
  );
}

function RoleChip({ role, active, disabled, onClick }: { role: string; active: boolean; disabled?: boolean; onClick: () => void }) {
  return (
    <button onClick={disabled ? undefined : onClick} className={cn(
      "rounded-full px-2 py-0.5 text-[10px] font-medium transition-colors",
      disabled && "cursor-default opacity-50",
      active ? "bg-primary text-primary-foreground" : "border border-border text-muted-foreground/60",
      !disabled && (active ? "hover:bg-primary/80" : "hover:border-muted-foreground hover:text-muted-foreground"),
    )}>
      {role}
    </button>
  );
}

function UserRow({ user, assignments, roles, isAdmin }: { user: User; assignments: Assignment[]; roles: Role[]; isAdmin: boolean }) {
  const assigned = new Set(assignments.filter((a) => a.userId === user.id).map((a) => a.role));
  return (
    <div className="flex items-center gap-2 px-3 py-1.5">
      <Users className="h-3.5 w-3.5 shrink-0 text-blue-400" />
      <span className="min-w-0 shrink-0 truncate text-xs">{user.displayName || user.username}</span>
      <div className="ml-auto flex flex-wrap items-center gap-1">
        {roles.map((r) => (
          <RoleChip key={r.name} role={r.name} active={assigned.has(r.name)} disabled={!isAdmin}
            onClick={() => assigned.has(r.name) ? revokeRole(user.id, r.name) : assignRole(user.id, r.name)} />
        ))}
      </div>
    </div>
  );
}

function PolicyGroup({ role, items }: { role: string; items: Policy[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button onClick={() => setOpen((o) => !o)} className={ROW} style={indent(1)}>
        {open ? <ChevronDown className={CHEVRON} /> : <ChevronRight className={CHEVRON} />}
        <Shield className="h-3.5 w-3.5 shrink-0 text-amber-400" />
        <span className="truncate">{role}</span>
        <span className="ml-auto shrink-0 pr-2 text-[10px] text-muted-foreground/50">{items.length}</span>
      </button>
      {open && items.map((p) => (
        <div key={p.entity} className="flex items-center gap-1 px-1 py-0.5 text-[10px] text-muted-foreground" style={indent(2)}>
          <KeyRound className="h-3 w-3 shrink-0 text-muted-foreground/60" />
          <span className="text-blue-400">{p.entity}</span>
          <span className="text-muted-foreground/50">[{p.actions.join(", ")}]{p.ownership && " owner"}</span>
        </div>
      ))}
    </div>
  );
}

export default function SecurityPanel() {
  const { projectPath } = useProjectContext();
  const { appId, roles, users, assignments, policies, loading, error, isAdmin } = useSyncExternalStore(subscribe, getSnapshot);

  useEffect(() => { if (projectPath) loadProject(projectPath); }, [projectPath]);

  if (!projectPath)
    return <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">No project opened</div>;

  const byRole = policies.reduce<Record<string, Policy[]>>((acc, p) => {
    (acc[p.role] ??= []).push(p);
    return acc;
  }, {});

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="truncate text-xs font-medium uppercase tracking-wider text-muted-foreground">{appId ?? "Security"}</span>
        <Button size="icon" variant="ghost" className="h-6 w-6" onClick={() => refresh()}>
          <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </Button>
      </div>

      {error && <div className="px-3 py-2 text-xs text-red-400">{error}</div>}
      {loading && roles.length === 0 && <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">Loading...</div>}
      {!loading && !error && roles.length === 0 && appId && <div className="flex items-center justify-center p-6 text-xs text-muted-foreground">No RBAC configured</div>}

      <div className="flex-1 overflow-y-auto py-1">
        {roles.length > 0 && (
          <Section title="Roles" icon={Shield} count={roles.length}>
            {roles.map((r) => <RoleNode key={r.name} role={r} depth={1} />)}
          </Section>
        )}
        {roles.length > 0 && (
          <Section title="Users" icon={Users} count={users.length}>
            {users.length > 0
              ? users.map((u) => <UserRow key={u.id} user={u} assignments={assignments} roles={roles} isAdmin={isAdmin} />)
              : <div className="px-3 py-2 text-[10px] text-muted-foreground">No users registered</div>}
          </Section>
        )}
        {Object.keys(byRole).length > 0 && (
          <Section title="Policies" icon={KeyRound} count={policies.length}>
            {Object.entries(byRole).map(([role, items]) => <PolicyGroup key={role} role={role} items={items} />)}
          </Section>
        )}
      </div>
    </div>
  );
}
