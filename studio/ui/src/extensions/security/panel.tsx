import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { Shield, Users, Plus, Trash2, RefreshCw, ChevronRight, Search, ArrowLeft, Info } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ListRow } from "@/components/ui/list-row";
import { Popover, PopoverTrigger, PopoverContent } from "@/components/ui/popover";
import { ToggleDot } from "@/components/ui/toggle-dot";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";
import {
  Dialog, DialogTrigger, DialogContent,
  DialogHeader, DialogBody, DialogFooter,
  DialogTitle, DialogDescription,
} from "@/components/ui/dialog";
import { useProjectContext } from "@/components/layout/app-context";
import {
  subscribe, getSnapshot, loadProject, refresh, assignRole, revokeRole,
  createRole, updateRole, deleteRole,
  type Role, type User, type Assignment, type PermissionDeclaration,
} from "./store";

// ── Helpers ─────────────────────────────────────────────────────────────────

function groupPermissions(perms: PermissionDeclaration[]) {
  const groups = new Map<string, PermissionDeclaration[]>();
  for (const p of perms) {
    const parts = p.key.split(":");
    const ns = parts.length >= 3 ? `${parts[0]}:${parts[1]}` : parts[0];
    if (!groups.has(ns)) groups.set(ns, []);
    groups.get(ns)!.push(p);
  }
  return [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]));
}

// ── Create Role Dialog ──────────────────────────────────────────────────────

function CreateRoleDialog({ onCreated }: { onCreated: (name: string) => void }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");

  const reset = () => { setName(""); setDescription(""); };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    createRole(trimmed, description.trim() || undefined);
    setOpen(false);
    reset();
    onCreated(trimmed);
  };

  return (
    <Dialog open={open} onOpenChange={(v) => { setOpen(v); if (!v) reset(); }}>
      <DialogTrigger asChild>
        <Button size="xs" variant="outline" className="w-full gap-1">
          <Plus className="h-3 w-3" /> New role
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-sm">
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Create role</DialogTitle>
            <DialogDescription>
              Roles group permissions and can be assigned to users.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="space-y-2.5">
            <div className="space-y-1">
              <Label htmlFor="role-name" size="xs">Name</Label>
              <Input
                id="role-name"
                autoFocus
                size="xs"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. editor"
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="role-desc" size="xs">Description</Label>
              <Input
                id="role-desc"
                size="xs"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="Optional"
              />
            </div>
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" size="xs" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button type="submit" size="xs" disabled={!name.trim()}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── Delete Confirmation ─────────────────────────────────────────────────────

function DeleteRoleButton({ roleName, onDeleted }: { roleName: string; onDeleted: () => void }) {
  const [open, setOpen] = useState(false);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button size="icon-xs" variant="ghost" className="text-muted-foreground hover:text-destructive">
          <Trash2 className="h-3 w-3" />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-xs">
        <DialogHeader>
          <DialogTitle>Delete role</DialogTitle>
          <DialogDescription>
            This will remove <strong>{roleName}</strong> and unassign it from all users. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button size="xs" variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button size="xs" variant="destructive" onClick={() => { deleteRole(roleName); setOpen(false); onDeleted(); }}>
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ── Sub-views ───────────────────────────────────────────────────────────────

type View = "users" | "roles" | { role: string };

function UsersView({ users, roles, assignments, isAdmin }: {
  users: User[]; roles: Role[]; assignments: Assignment[]; isAdmin: boolean;
}) {
  const assignedMap = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const a of assignments) {
      if (!m.has(a.userId)) m.set(a.userId, []);
      m.get(a.userId)!.push(a.role);
    }
    return m;
  }, [assignments]);

  if (users.length === 0) {
    return <div className="flex items-center justify-center p-6 text-[10px] text-muted-foreground">No users registered</div>;
  }

  const toggleRole = (userId: string, role: string, hasIt: boolean) => {
    if (hasIt) revokeRole(userId, role);
    else assignRole(userId, role);
  };

  return (
    <div className="space-y-1 p-2">
      {users.map((u) => {
        const userRoles = new Set(assignedMap.get(u.id) ?? []);
        return (
          <Popover key={u.id}>
            <PopoverTrigger asChild>
              <div>
                <ListRow className="cursor-pointer">
                  <span className="min-w-[80px] truncate text-xs" title={u.email}>
                    {u.displayName || u.email}
                  </span>
                  <div className="flex flex-1 flex-wrap items-center gap-1">
                    {[...userRoles].map((r) => (
                      <span
                        key={r}
                        className={cn(
                          "rounded-full px-2 py-0.5 text-[10px] font-medium",
                          r === "admin" ? "bg-amber-500/20 text-amber-400" : "bg-accent text-accent-foreground",
                        )}
                      >
                        {r}
                      </span>
                    ))}
                    {userRoles.size === 0 && (
                      <span className="text-[10px] text-muted-foreground">No roles</span>
                    )}
                  </div>
                  <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
                </ListRow>
              </div>
            </PopoverTrigger>
            <PopoverContent className="min-w-[160px]">
              <div className="px-2 py-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                Roles for {u.displayName || u.email}
              </div>
              {roles.map((r) => {
                const hasIt = userRoles.has(r.name);
                return (
                  <ListRow
                    key={r.name}
                    onClick={isAdmin ? () => toggleRole(u.id, r.name, hasIt) : undefined}
                  >
                    <ToggleDot active={hasIt} disabled={!isAdmin} />
                    <Shield className={cn("h-3 w-3 shrink-0", r.name === "admin" ? "text-amber-400" : "text-muted-foreground")} />
                    <span className="text-xs">{r.name}</span>
                  </ListRow>
                );
              })}
            </PopoverContent>
          </Popover>
        );
      })}
    </div>
  );
}

function RolesListView({ roles, isAdmin, onSelect, onCreated }: {
  roles: Role[]; isAdmin: boolean;
  onSelect: (name: string) => void; onCreated: (name: string) => void;
}) {
  return (
    <div className="space-y-1 p-2">
      {roles.map((r) => (
        <ListRow key={r.name} onClick={() => onSelect(r.name)}>
          <Shield className={cn("h-3.5 w-3.5 shrink-0", r.name === "admin" ? "text-amber-400" : "text-muted-foreground")} />
          <div className="flex-1 min-w-0">
            <div className="text-xs font-medium truncate">{r.name}</div>
            {r.description && <div className="text-[10px] text-muted-foreground truncate">{r.description}</div>}
          </div>
          <span className="shrink-0 rounded-full bg-accent px-1.5 py-0.5 text-[9px] text-muted-foreground">
            {r.permissions.includes("*") ? "all" : r.permissions.length}
          </span>
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
        </ListRow>
      ))}
      {isAdmin && <CreateRoleDialog onCreated={onCreated} />}
    </div>
  );
}

function RoleDetailView({ role, availablePermissions, isAdmin, onBack }: {
  role: Role; availablePermissions: PermissionDeclaration[];
  isAdmin: boolean; onBack: () => void;
}) {
  const [search, setSearch] = useState("");
  const isBuiltIn = role.name === "admin";
  const isWildcard = role.permissions.includes("*");

  const scopedWildcards = useMemo(
    () => role.permissions.filter((p) => p.endsWith(":*")),
    [role.permissions],
  );

  const coveredByWildcard = useMemo(() => {
    const prefixes = scopedWildcards.map((p) => p.slice(0, -1));
    return (key: string) => isWildcard || prefixes.some((w) => key.startsWith(w));
  }, [scopedWildcards, isWildcard]);

  const wildcardForGroup = (ns: string) =>
    scopedWildcards.find((w) => w === `${ns}:*` || ns.startsWith(w.slice(0, -2)));

  const grouped = useMemo(() => groupPermissions(availablePermissions), [availablePermissions]);

  const filtered = useMemo(() => {
    if (!search) return grouped;
    const q = search.toLowerCase();
    return grouped
      .map(([ns, perms]) => [ns, perms.filter((p) => p.key.toLowerCase().includes(q) || p.description.toLowerCase().includes(q))] as const)
      .filter(([, perms]) => perms.length > 0)
      .map(([ns, perms]) => [ns, [...perms]] as [string, PermissionDeclaration[]]);
  }, [grouped, search]);

  const togglePermission = (key: string) => {
    if (isBuiltIn || !isAdmin || coveredByWildcard(key)) return;
    const next = role.permissions.includes(key)
      ? role.permissions.filter((p) => p !== key)
      : [...role.permissions, key];
    updateRole(role.name, { permissions: next });
  };

  const toggleGroup = (permsInGroup: PermissionDeclaration[]) => {
    if (isBuiltIn || !isAdmin) return;
    const toggleable = permsInGroup.filter((p) => !coveredByWildcard(p.key));
    if (toggleable.length === 0) return;
    const keys = toggleable.map((p) => p.key);
    const allSelected = keys.every((k) => role.permissions.includes(k));
    let next: string[];
    if (allSelected) {
      next = role.permissions.filter((p) => !keys.includes(p));
    } else {
      const current = new Set(role.permissions);
      keys.forEach((k) => current.add(k));
      next = [...current];
    }
    updateRole(role.name, { permissions: next });
  };

  const expandWildcard = (ns: string, permsInGroup: PermissionDeclaration[]) => {
    const wc = wildcardForGroup(ns);
    if (!wc) return;
    const next = role.permissions.filter((p) => p !== wc);
    permsInGroup.forEach((p) => { if (!next.includes(p.key)) next.push(p.key); });
    updateRole(role.name, { permissions: next });
  };

  const collapseToWildcard = (ns: string, permsInGroup: PermissionDeclaration[]) => {
    const keys = new Set(permsInGroup.map((p) => p.key));
    const wc = `${ns}:*`;
    const next = role.permissions.filter((p) => !keys.has(p));
    if (!next.includes(wc)) next.push(wc);
    updateRole(role.name, { permissions: next });
  };

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-2 py-1.5">
        <Button size="icon-xs" variant="ghost" onClick={onBack}>
          <ArrowLeft className="h-3.5 w-3.5" />
        </Button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium truncate">
            {role.name}
            {isBuiltIn && <span className="ml-1 text-[9px] text-amber-400">(built-in)</span>}
          </div>
          {role.description && <div className="text-[10px] text-muted-foreground truncate">{role.description}</div>}
        </div>
        {!isBuiltIn && isAdmin && (
          <DeleteRoleButton roleName={role.name} onDeleted={onBack} />
        )}
      </div>

      {/* Inherits */}
      {role.inherits.length > 0 && (
        <div className="shrink-0 border-b border-border px-3 py-1.5 text-[10px] text-muted-foreground">
          Inherits: {role.inherits.join(", ")}
        </div>
      )}

      {/* Search */}
      <div className="shrink-0 border-b border-border px-2 py-1">
        <div className="relative">
          <Search className="absolute left-1.5 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Filter permissions…"
            size="xs"
            className="pl-6"
          />
        </div>
      </div>

      {/* Wildcard toggle */}
      {!isBuiltIn && isAdmin && (
        <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
          <span className="text-[10px] text-muted-foreground">
            {isWildcard ? "All permissions granted" : "Granular permissions"}
          </span>
          <Button size="xs" variant="outline" className="text-[10px]"
            onClick={() => updateRole(role.name, {
              permissions: isWildcard ? availablePermissions.map(p => p.key) : ["*"]
            })}>
            {isWildcard ? "Restrict" : "Grant all"}
          </Button>
        </div>
      )}
      {isBuiltIn && isWildcard && (
        <div className="shrink-0 border-b border-border px-3 py-1.5 text-[10px] text-amber-400">
          All permissions granted (built-in)
        </div>
      )}

      {/* Permission groups */}
      <div className="flex-1 overflow-auto">
        {filtered.map(([ns, perms]) => {
          const selectedCount = perms.filter((p) => coveredByWildcard(p.key) || role.permissions.includes(p.key)).length;
          const groupWc = wildcardForGroup(ns);
          return (
            <div key={ns} className="border-b border-border/50">
              <div className="flex items-center gap-2 px-3 py-1.5">
                <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">{ns}</span>
                <span className="text-[10px] text-muted-foreground">{selectedCount}/{perms.length}</span>
                {!isBuiltIn && isAdmin && (
                  groupWc ? (
                    <Button size="xs" variant="link" className="ml-auto" onClick={() => expandWildcard(ns, perms)}>
                      Customize
                    </Button>
                  ) : (
                    <div className="ml-auto flex items-center gap-1">
                      {selectedCount === perms.length && (
                        <Button size="xs" variant="link" onClick={() => collapseToWildcard(ns, perms)}>
                          Grant all
                        </Button>
                      )}
                      <Button size="xs" variant="link" onClick={() => toggleGroup(perms)}>
                        {selectedCount === perms.length ? "Clear" : "Select all"}
                      </Button>
                    </div>
                  )
                )}
              </div>
              {perms.map((p) => {
                const wildcarded = coveredByWildcard(p.key);
                const canToggle = !isBuiltIn && isAdmin && !wildcarded;
                return (
                  <ListRow
                    key={p.key}
                    onClick={canToggle ? () => togglePermission(p.key) : undefined}
                  >
                    <ToggleDot
                      active={wildcarded || role.permissions.includes(p.key)}
                      disabled={!canToggle}
                    />
                    <code className="flex-1 min-w-0 truncate text-xs">{p.key}</code>
                    {p.description && (
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Info className="h-3 w-3 shrink-0 text-muted-foreground/50 hover:text-muted-foreground" />
                        </TooltipTrigger>
                        <TooltipContent side="top" className="max-w-xs text-xs">
                          {p.description}
                        </TooltipContent>
                      </Tooltip>
                    )}
                  </ListRow>
                );
              })}
            </div>
          );
        })}
        {filtered.length === 0 && availablePermissions.length > 0 && (
          <div className="p-3 text-[10px] text-muted-foreground">No permissions match your filter.</div>
        )}
        {availablePermissions.length === 0 && (
          <div className="p-3 text-[10px] text-muted-foreground">No permissions declared in manifest.</div>
        )}
      </div>
    </div>
  );
}

// ── Main panel ──────────────────────────────────────────────────────────────

export default function SecurityPanel() {
  const { projectPath } = useProjectContext();
  const { roles, users, assignments, availablePermissions, loading, error, isAdmin } = useSyncExternalStore(subscribe, getSnapshot);

  const [view, setView] = useState<View>("users");

  useEffect(() => { if (projectPath) loadProject(); }, [projectPath]);

  if (!projectPath) {
    return <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">No project opened</div>;
  }

  const selectedRole = typeof view === "object" ? roles.find((r) => r.name === view.role) : null;

  return (
    <TooltipProvider delayDuration={200}>
      <div className="flex h-full flex-col">
        {/* Top bar */}
        <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
          <span className="truncate text-xs font-medium uppercase tracking-wider text-muted-foreground">Security</span>
          <Button size="icon-xs" variant="ghost" onClick={() => refresh()}>
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
          </Button>
        </div>

        {error && <div className="px-3 py-2 text-xs text-red-400">{error}</div>}
        {loading && roles.length === 0 && <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">Loading...</div>}
        {!loading && !error && roles.length === 0 && <div className="flex items-center justify-center p-6 text-xs text-muted-foreground">No RBAC configured</div>}

        {roles.length > 0 && !selectedRole && (
          <>
            {/* Nav tabs */}
            <div className="flex shrink-0 border-b border-border">
              <button
                onClick={() => setView("users")}
                className={cn(
                  "flex-1 px-3 py-1.5 text-[10px] font-medium uppercase tracking-wider",
                  view === "users" ? "border-b-2 border-primary text-foreground" : "text-muted-foreground hover:text-foreground",
                )}
              >
                <Users className="mr-1 inline h-3 w-3" />Users
              </button>
              <button
                onClick={() => setView("roles")}
                className={cn(
                  "flex-1 px-3 py-1.5 text-[10px] font-medium uppercase tracking-wider",
                  view === "roles" ? "border-b-2 border-primary text-foreground" : "text-muted-foreground hover:text-foreground",
                )}
              >
                <Shield className="mr-1 inline h-3 w-3" />Roles
              </button>
            </div>

            {/* View content */}
            <div className="flex-1 overflow-auto">
              {view === "users" && (
                <UsersView users={users} roles={roles} assignments={assignments} isAdmin={isAdmin} />
              )}
              {view === "roles" && (
                <RolesListView
                  roles={roles}
                  isAdmin={isAdmin}
                  onSelect={(name) => setView({ role: name })}
                  onCreated={(name) => setView({ role: name })}
                />
              )}
            </div>
          </>
        )}

        {selectedRole && (
          <RoleDetailView
            role={selectedRole}
            availablePermissions={availablePermissions}
            isAdmin={isAdmin}
            onBack={() => setView("roles")}
          />
        )}
      </div>
    </TooltipProvider>
  );
}
