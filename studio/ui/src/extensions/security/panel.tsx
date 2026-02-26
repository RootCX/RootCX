import { useEffect, useSyncExternalStore } from "react";
import { Shield, Users, KeyRound, RefreshCw, Lock } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";
import { useProjectContext } from "@/components/layout/app-context";
import {
  subscribe, getSnapshot, loadProject, refresh, assignRole, revokeRole,
  type Role, type User, type Assignment, type Policy,
} from "./store";

const ALL_ACTIONS = ["create", "read", "update", "delete"] as const;

function ToggleCell({ active, disabled, onClick }: { active: boolean; disabled: boolean; onClick: () => void }) {
  return (
    <button
      onClick={disabled ? undefined : onClick}
      className={cn(
        "flex h-6 w-6 items-center justify-center rounded transition-colors",
        disabled ? "cursor-default" : "cursor-pointer",
        !disabled && "hover:bg-accent",
      )}
    >
      <span className={cn(
        "h-3 w-3 rounded-full border-2 transition-colors",
        active
          ? "border-primary bg-primary"
          : disabled
            ? "border-muted-foreground/30"
            : "border-muted-foreground/50 hover:border-muted-foreground",
      )} />
    </button>
  );
}

function AssignmentMatrix({ users, roles, assignments, isAdmin }: {
  users: User[]; roles: Role[]; assignments: Assignment[]; isAdmin: boolean;
}) {
  const assignedSet = new Set(assignments.map((a) => `${a.userId}::${a.role}`));

  if (users.length === 0) {
    return <div className="flex items-center justify-center p-6 text-[10px] text-muted-foreground">No users registered</div>;
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-[10px]">
        <thead>
          <tr>
            <th className="sticky left-0 z-10 bg-background px-2 py-1.5 text-left font-medium text-muted-foreground">
              <Users className="inline h-3 w-3 mr-1" />User
            </th>
            {roles.map((r) => (
              <th key={r.name} className="px-1 py-1.5 text-center font-medium text-muted-foreground">
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="cursor-default">{r.name}</span>
                  </TooltipTrigger>
                  {r.description && (
                    <TooltipContent side="top">
                      <p>{r.description}</p>
                      {r.inherits.length > 0 && <p className="mt-0.5 opacity-70">Inherits: {r.inherits.join(", ")}</p>}
                    </TooltipContent>
                  )}
                </Tooltip>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <tr key={u.id} className="border-t border-border/50 hover:bg-accent/30">
              <td className="sticky left-0 z-10 bg-background px-2 py-0.5 text-xs">
                <span className="truncate block max-w-[100px]" title={u.username}>
                  {u.displayName || u.username}
                </span>
              </td>
              {roles.map((r) => {
                const key = `${u.id}::${r.name}`;
                const active = assignedSet.has(key);
                return (
                  <td key={r.name} className="px-1 py-0.5">
                    <ToggleCell
                      active={active}
                      disabled={!isAdmin}
                      onClick={() => active ? revokeRole(u.id, r.name) : assignRole(u.id, r.name)}
                    />
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ActionCell({ granted, ownership }: { granted: Set<string>; ownership: boolean }) {
  const hasWildcard = granted.has("*");
  return (
    <div className="flex items-center justify-center gap-px">
      {ALL_ACTIONS.map((a) => (
        <span key={a} className={cn(
          "inline-block w-3 text-center font-mono text-[9px] leading-none",
          hasWildcard || granted.has(a) ? "text-foreground font-semibold" : "text-muted-foreground/25",
        )}>
          {a[0]}
        </span>
      ))}
      {ownership && (
        <Tooltip>
          <TooltipTrigger asChild>
            <Lock className="ml-0.5 h-2.5 w-2.5 text-amber-400/70" />
          </TooltipTrigger>
          <TooltipContent side="top">Owner-restricted</TooltipContent>
        </Tooltip>
      )}
    </div>
  );
}

function PolicyMatrix({ roles, policies }: { roles: Role[]; policies: Policy[] }) {
  const entities = [...new Set(policies.map((p) => p.entity))].sort();
  const policyMap = new Map<string, Map<string, { actions: Set<string>; ownership: boolean }>>();
  for (const p of policies) {
    if (!policyMap.has(p.role)) policyMap.set(p.role, new Map());
    policyMap.get(p.role)!.set(p.entity, { actions: new Set(p.actions), ownership: p.ownership });
  }

  if (entities.length === 0) {
    return <div className="flex items-center justify-center p-6 text-[10px] text-muted-foreground">No policies configured</div>;
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-[10px]">
        <thead>
          <tr>
            <th className="sticky left-0 z-10 bg-background px-2 py-1.5 text-left font-medium text-muted-foreground">
              <Shield className="inline h-3 w-3 mr-1" />Role
            </th>
            {entities.map((e) => (
              <th key={e} className="px-1 py-1.5 text-center font-medium text-muted-foreground">
                {e === "*" ? <span className="text-amber-400">all</span> : e}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {roles.map((r) => {
            const roleMap = policyMap.get(r.name);
            return (
              <tr key={r.name} className="border-t border-border/50 hover:bg-accent/30">
                <td className="sticky left-0 z-10 bg-background px-2 py-0.5 text-xs font-medium">
                  {r.name}
                </td>
                {entities.map((e) => {
                  const cell = roleMap?.get(e);
                  return (
                    <td key={e} className="px-1 py-0.5 text-center">
                      {cell
                        ? <ActionCell granted={cell.actions} ownership={cell.ownership} />
                        : <span className="text-muted-foreground/20">—</span>}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default function SecurityPanel() {
  const { projectPath } = useProjectContext();
  const { appId, roles, users, assignments, policies, loading, error, isAdmin } = useSyncExternalStore(subscribe, getSnapshot);

  useEffect(() => { if (projectPath) loadProject(projectPath); }, [projectPath]);

  if (!projectPath) {
    return <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">No project opened</div>;
  }

  return (
    <TooltipProvider delayDuration={200}>
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

        {roles.length > 0 && (
          <Tabs defaultValue="assignments" className="flex flex-1 flex-col overflow-hidden">
            <TabsList className="shrink-0 border-b border-border px-1">
              <TabsTrigger value="assignments" className="gap-1">
                <Users className="h-3 w-3" />Assignments
              </TabsTrigger>
              <TabsTrigger value="policies" className="gap-1">
                <KeyRound className="h-3 w-3" />Policies
              </TabsTrigger>
            </TabsList>
            <TabsContent value="assignments" className="flex-1 overflow-auto p-1">
              <AssignmentMatrix users={users} roles={roles} assignments={assignments} isAdmin={isAdmin} />
            </TabsContent>
            <TabsContent value="policies" className="flex-1 overflow-auto p-1">
              <PolicyMatrix roles={roles} policies={policies} />
            </TabsContent>
          </Tabs>
        )}
      </div>
    </TooltipProvider>
  );
}
