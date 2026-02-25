import { useState, useEffect, useSyncExternalStore } from "react";
import { ChevronRight, ChevronDown, Shield, Users, KeyRound, UserPlus, X, RefreshCw } from "lucide-react";
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

function AssignPicker({ userId, available, onDone }: { userId: string; available: Role[]; onDone: () => void }) {
  return (
    <div className="flex flex-col gap-0.5 px-1 py-0.5">
      {available.map((r) => (
        <button key={r.name} onClick={() => { assignRole(userId, r.name); onDone(); }}
          className="rounded px-1 py-0.5 text-left text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground">
          + {r.name}
        </button>
      ))}
      <button onClick={onDone} className="px-1 py-0.5 text-[10px] text-muted-foreground/50 hover:text-muted-foreground">Cancel</button>
    </div>
  );
}

function UserNode({ user, assignments, roles, depth }: { user: User; assignments: Assignment[]; roles: Role[]; depth: number }) {
  const [expanded, setExpanded] = useState(false);
  const [picking, setPicking] = useState(false);
  const mine = assignments.filter((a) => a.userId === user.id);
  const available = roles.filter((r) => !mine.some((a) => a.role === r.name));

  return (
    <div>
      <button onClick={() => setExpanded((e) => !e)} className={ROW} style={indent(depth)}>
        {expanded ? <ChevronDown className={CHEVRON} /> : <ChevronRight className={CHEVRON} />}
        <Users className="h-3.5 w-3.5 shrink-0 text-blue-400" />
        <span className="truncate">{user.displayName || user.username}</span>
        <span className="ml-auto shrink-0 pr-2 text-[10px] text-muted-foreground/50">
          {mine.length} role{mine.length !== 1 ? "s" : ""}
        </span>
      </button>
      {expanded && (
        <div style={indent(depth + 1)}>
          {mine.map((a) => (
            <div key={a.role} className="group flex items-center gap-1 px-1 py-0.5 text-[10px] text-muted-foreground">
              <KeyRound className="h-3 w-3 shrink-0 text-muted-foreground/60" />
              <span>{a.role}</span>
              <button onClick={() => revokeRole(user.id, a.role)} title="Revoke"
                className="ml-auto opacity-0 transition-opacity group-hover:opacity-100 hover:text-red-400">
                <X className="h-3 w-3" />
              </button>
            </div>
          ))}
          {available.length > 0 && (picking
            ? <AssignPicker userId={user.id} available={available} onDone={() => setPicking(false)} />
            : <button onClick={() => setPicking(true)} className="flex items-center gap-1 px-1 py-0.5 text-[10px] text-muted-foreground hover:text-foreground">
                <UserPlus className="h-3 w-3" /> Assign role
              </button>
          )}
        </div>
      )}
    </div>
  );
}

function PolicyRow({ policy, depth }: { policy: Policy; depth: number }) {
  return (
    <div className="flex w-full items-center gap-1 px-1 py-0.5 text-xs text-muted-foreground" style={indent(depth)}>
      <span className="w-3.5 shrink-0" />
      <KeyRound className="h-3 w-3 shrink-0 text-muted-foreground/60" />
      <span className="truncate">
        <span className="text-foreground">{policy.role}</span>{" \u2192 "}
        <span className="text-blue-400">{policy.entity}</span>
        <span className="ml-1 text-[10px] text-muted-foreground/50">
          [{policy.actions.join(", ")}]{policy.ownership && " owner"}
        </span>
      </span>
    </div>
  );
}

export default function SecurityPanel() {
  const { projectPath } = useProjectContext();
  const { appId, roles, users, assignments, policies, loading, error } = useSyncExternalStore(subscribe, getSnapshot);

  useEffect(() => { if (projectPath) loadProject(projectPath); }, [projectPath]);

  if (!projectPath)
    return <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">No project opened</div>;

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
        {users.length > 0 && (
          <Section title="Users" icon={Users} count={users.length}>
            {users.map((u) => <UserNode key={u.id} user={u} assignments={assignments} roles={roles} depth={1} />)}
          </Section>
        )}
        {policies.length > 0 && (
          <Section title="Policies" icon={KeyRound} count={policies.length}>
            {policies.map((p, i) => <PolicyRow key={`${p.role}-${p.entity}-${i}`} policy={p} depth={1} />)}
          </Section>
        )}
      </div>
    </div>
  );
}
