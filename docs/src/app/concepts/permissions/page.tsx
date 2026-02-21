import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "RBAC Model Overview" },
    { id: "roles", title: "Roles" },
    { id: "role-inheritance", title: "Role Inheritance" },
    { id: "default-role", title: "Default Role" },
    { id: "policies", title: "Policies" },
    { id: "actions", title: "Actions" },
    { id: "wildcard-entity", title: "Wildcard Entity" },
    { id: "ownership", title: "Ownership & Row-Level Filtering" },
    { id: "evaluation", title: "Policy Evaluation Algorithm" },
    { id: "rbac-routes", title: "RBAC API Routes" },
    { id: "complete-example", title: "Complete Example" },
    { id: "database-schema", title: "Database Schema" },
    { id: "related", title: "Related Pages" },
];

export default function PermissionsPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/concepts/manifest" className="hover:text-foreground transition-colors">Core Concepts</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Roles & Permissions</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Roles & Permissions</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        RootCX uses a Role-Based Access Control (RBAC) model to govern who can perform which operations on which entities. Roles, policies, and ownership rules are all declared in the App Manifest and enforced automatically by the runtime before any SQL reaches the database.
                    </p>
                </header>

                {/* RBAC Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">RBAC Model Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        <strong className="text-foreground font-medium">Role-Based Access Control (RBAC)</strong> is an access control model where permissions are not granted directly to users — they are granted to <em>roles</em>, and roles are assigned to users. A user's effective permissions are the union of the permissions of all roles assigned to them (plus inherited roles).
                    </p>
                    <p className="text-muted-foreground leading-7">
                        In RootCX, the RBAC configuration lives in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permissions</code> block of the App Manifest. The three core concepts are:
                    </p>

                    <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
                        {[
                            {
                                term: "Roles",
                                def: "Named groups that represent a user's function or trust level within the application. Examples: admin, manager, viewer, external_auditor.",
                            },
                            {
                                term: "Policies",
                                def: "Rules that grant a role the ability to perform specific CRUD actions on a specific entity. Policies are additive — there are no deny rules.",
                            },
                            {
                                term: "Assignments",
                                def: "Runtime records that map a user to one or more roles. Managed via the RBAC API, not the manifest. A user can have multiple role assignments.",
                            },
                        ].map(({ term, def }) => (
                            <div key={term} className="rounded-lg border border-border bg-white/[0.02] p-4">
                                <p className="text-sm font-semibold text-foreground mb-2">{term}</p>
                                <p className="text-xs text-muted-foreground leading-relaxed">{def}</p>
                            </div>
                        ))}
                    </div>

                    <p className="text-muted-foreground leading-7">
                        When a request arrives at a data API endpoint, the runtime extracts the authenticated user's identity from the JWT, resolves their role assignments, walks the role inheritance graph, collects the union of all permitted actions, and either allows or rejects the operation — all before the database query is constructed.
                    </p>

                    <Callout variant="info" title="RBAC is manifest-driven">
                        Roles and policies are declared in the manifest and applied on every deployment. Role <em>assignments</em> (associating a user with a role) are managed at runtime through the RBAC API and persisted in the system schema — they survive redeployments.
                    </Callout>
                </section>

                {/* Roles */}
                <section className="flex flex-col gap-4" id="roles">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Roles</h2>
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">role</strong> is defined in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permissions.roles</code> array. Each role has a name and an optional list of parent roles it inherits from. Roles themselves carry no permissions — permissions are granted through <em>policies</em> that reference the role by name.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "name",
                            type: "string",
                            required: true,
                            description: "Unique identifier for the role within the application. Used in policy definitions and RBAC API calls. Lowercase alphanumeric with hyphens recommended.",
                        },
                        {
                            name: "inherits",
                            type: "string[]",
                            required: false,
                            default: "[]",
                            description: "Names of roles this role inherits from. The runtime resolves the full inheritance graph and collects the union of all permitted actions from the role and all its ancestors.",
                        },
                    ]} />

                    <CodeBlock language="json" filename="Roles definition" code={`"roles": [
  { "name": "admin" },
  { "name": "manager",  "inherits": ["member"] },
  { "name": "member" },
  { "name": "auditor" },
  { "name": "readonly", "inherits": ["auditor"] }
]`} />

                    <Callout variant="tip" title="Role naming conventions">
                        Role names are case-sensitive. Use lowercase and avoid spaces. Hyphens are permitted (e.g. <code>account-manager</code>). Keep role names stable — renaming a role in the manifest will cause the old role to be orphaned in the database unless you migrate existing assignments.
                    </Callout>
                </section>

                {/* Role Inheritance */}
                <section className="flex flex-col gap-4" id="role-inheritance">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Role Inheritance</h2>
                    <p className="text-muted-foreground leading-7">
                        Role inheritance allows you to build permission hierarchies without duplicating policy definitions. When role <strong className="text-foreground font-medium">A</strong> inherits from role <strong className="text-foreground font-medium">B</strong>, users with role A gain all permissions granted to role B — in addition to A's own policies.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The runtime resolves inheritance using a <strong className="text-foreground font-medium">breadth-first traversal</strong> of the inheritance graph. Circular inheritance is detected during manifest validation and rejected before deployment. Multi-level and multiple inheritance are both supported.
                    </p>

                    <CodeBlock language="json" filename="Multi-level inheritance example" code={`"roles": [
  { "name": "superadmin",  "inherits": ["admin"] },
  { "name": "admin",       "inherits": ["manager"] },
  { "name": "manager",     "inherits": ["member"] },
  { "name": "member" }
]`} />

                    <p className="text-muted-foreground leading-7">
                        In this example, a user assigned the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">superadmin</code> role effectively has the permissions of <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">superadmin</code> + <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">admin</code> + <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">manager</code> + <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">member</code>. Multiple inheritance is equally supported:
                    </p>

                    <CodeBlock language="json" filename="Multiple inheritance example" code={`"roles": [
  { "name": "team-lead",  "inherits": ["developer", "reviewer"] },
  { "name": "developer" },
  { "name": "reviewer" }
]`} />

                    <Callout variant="warning" title="Inheritance is additive only">
                        There are no deny policies in RootCX. Inheritance can only expand the set of granted permissions — it can never restrict them. If a parent role has <code>delete</code> on an entity, all child roles also have <code>delete</code>, and there is no way to remove it through inheritance. Design your hierarchy from least to most privileged.
                    </Callout>
                </section>

                {/* Default Role */}
                <section className="flex flex-col gap-4" id="default-role">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Default Role</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permissions.defaultRole</code> field specifies the role that is automatically assigned to every new user when they register via the authentication API.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The default role assignment is created immediately after the user record is inserted. It functions identically to a role assigned manually via the RBAC API — the user can be granted additional roles later, and the default role assignment can also be revoked after a different role has been assigned.
                    </p>

                    <CodeBlock language="json" filename="Default role" code={`"permissions": {
  "defaultRole": "member",
  ...
}`} />

                    <Callout variant="note" title="Choosing a safe default role">
                        The default role should be the most restrictive role in your hierarchy — typically a role with only <code>create</code> and <code>read</code> permissions on a limited set of entities, and always with <code>ownerOnly: true</code>. This ensures new users start with minimal access and are granted elevated roles explicitly.
                    </Callout>
                </section>

                {/* Policies */}
                <section className="flex flex-col gap-4" id="policies">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Policies</h2>
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">policy</strong> is the fundamental unit of access control in RootCX. It grants a named role the ability to perform a set of CRUD actions on a specific entity (or all entities via the wildcard). Policies are declared in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permissions.policies</code> array and stored in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system.rbac_policies</code> table after deployment.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "role",
                            type: "string",
                            required: true,
                            description: "The role name this policy applies to. Must exactly match a role name in the roles array.",
                        },
                        {
                            name: "entity",
                            type: "string",
                            required: true,
                            description: "The entityName this policy covers. Must match an entityName in the dataContract array, or be \"*\" to apply to all entities.",
                        },
                        {
                            name: "actions",
                            type: "string[]",
                            required: true,
                            description: "Array of permitted actions. Valid values: \"create\", \"read\", \"update\", \"delete\". An empty array is valid but grants no permissions.",
                        },
                        {
                            name: "ownerOnly",
                            type: "boolean",
                            required: false,
                            default: "false",
                            description: "When true, read/update/delete operations are automatically filtered to rows where owner_id equals the authenticated user's ID. Inserts always set owner_id to the user's ID regardless.",
                        },
                    ]} />

                    <CodeBlock language="json" filename="Policy examples" code={`"policies": [
  {
    "role": "admin",
    "entity": "*",
    "actions": ["create", "read", "update", "delete"]
  },
  {
    "role": "manager",
    "entity": "contacts",
    "actions": ["create", "read", "update", "delete"]
  },
  {
    "role": "member",
    "entity": "contacts",
    "actions": ["create", "read", "update"],
    "ownerOnly": true
  },
  {
    "role": "auditor",
    "entity": "*",
    "actions": ["read"]
  }
]`} />
                </section>

                {/* Actions */}
                <section className="flex flex-col gap-4" id="actions">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Actions</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">actions</code> array in a policy grants specific CRUD operations. Each action maps to one or more HTTP methods on the Data API.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Action</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">HTTP Methods</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">SQL Operation</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Description</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-green-400">create</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">POST</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">INSERT</td>
                                    <td className="px-4 py-3 text-muted-foreground text-sm">Create a new record. The owner_id is automatically set to the authenticated user's ID.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-blue-400">read</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">GET</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">SELECT</td>
                                    <td className="px-4 py-3 text-muted-foreground text-sm">List all records or fetch a single record by ID. With ownerOnly, filtered to the user's own records.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-yellow-400">update</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">PATCH, PUT</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">UPDATE</td>
                                    <td className="px-4 py-3 text-muted-foreground text-sm">Modify an existing record by ID. With ownerOnly, only records owned by the user can be modified.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-red-400">delete</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DELETE</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DELETE</td>
                                    <td className="px-4 py-3 text-muted-foreground text-sm">Delete a record by ID. With ownerOnly, only the user's own records can be deleted.</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>

                    <Callout variant="info" title="Action granularity">
                        You can grant any subset of actions. A common pattern is to grant <code>read</code> broadly (e.g. to all authenticated users) and restrict <code>delete</code> to admin roles only. A user with no policies granting a given action will receive a <code>403 Forbidden</code> response from the Data API.
                    </Callout>
                </section>

                {/* Wildcard Entity */}
                <section className="flex flex-col gap-4" id="wildcard-entity">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Wildcard Entity</h2>
                    <p className="text-muted-foreground leading-7">
                        Setting <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">entity</code> to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">"*"</code> in a policy creates a <strong className="text-foreground font-medium">blanket policy</strong> that applies across all entities defined in the data contract. This is the canonical way to define admin-level access without enumerating every entity.
                    </p>

                    <CodeBlock language="json" filename="Wildcard policy" code={`{
  "role": "admin",
  "entity": "*",
  "actions": ["create", "read", "update", "delete"]
}`} />

                    <p className="text-muted-foreground leading-7">
                        Wildcard policies and entity-specific policies coexist. During evaluation, the runtime collects all matching policies (wildcard + specific entity) for the user's roles and takes the union of their actions. A specific entity policy can therefore <em>extend</em> a wildcard policy but cannot restrict it.
                    </p>

                    <Callout variant="tip" title="Combining wildcard and specific policies">
                        A common pattern is to give a base role a wildcard <code>read</code> policy and then add specific <code>create</code>/<code>update</code>/<code>delete</code> policies for entities they own. This grants universal read access while keeping write access entity-scoped.
                    </Callout>
                </section>

                {/* Ownership */}
                <section className="flex flex-col gap-4" id="ownership">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Ownership & Row-Level Filtering</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX implements <strong className="text-foreground font-medium">row-level ownership</strong> through the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ownerOnly</code> policy flag and the system-managed <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id</code> column present on every entity table.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        When <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ownerOnly: true</code> is set on a policy:
                    </p>

                    <ul className="flex flex-col gap-3 text-muted-foreground leading-7">
                        <li className="flex gap-3">
                            <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-primary" />
                            <span><strong className="text-foreground font-medium">INSERT (create):</strong> The runtime automatically sets <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id</code> to the authenticated user's ID. The client cannot override this value.</span>
                        </li>
                        <li className="flex gap-3">
                            <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-primary" />
                            <span><strong className="text-foreground font-medium">SELECT (read):</strong> The runtime appends <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">WHERE owner_id = $user_id</code> to the query. Users can only see their own rows.</span>
                        </li>
                        <li className="flex gap-3">
                            <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-primary" />
                            <span><strong className="text-foreground font-medium">UPDATE:</strong> The runtime appends <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">WHERE id = $record_id AND owner_id = $user_id</code>. Updates to records not owned by the user silently affect 0 rows and return a 404.</span>
                        </li>
                        <li className="flex gap-3">
                            <span className="mt-1 h-1.5 w-1.5 shrink-0 rounded-full bg-primary" />
                            <span><strong className="text-foreground font-medium">DELETE:</strong> Same as UPDATE — the ownership filter is appended to prevent deletion of records not owned by the user.</span>
                        </li>
                    </ul>

                    <CodeBlock language="sql" filename="Generated SQL with ownerOnly" code={`-- GET /api/v1/data/contacts (ownerOnly: true)
SELECT * FROM crm_app.contacts
WHERE owner_id = '3f7a1b2c-...'   -- authenticated user's ID
ORDER BY created_at DESC;

-- PATCH /api/v1/data/contacts/:id (ownerOnly: true)
UPDATE crm_app.contacts
SET first_name = 'Alice', updated_at = NOW()
WHERE id = 'a1b2c3d4-...'
  AND owner_id = '3f7a1b2c-...'; -- prevents cross-user updates`} />

                    <Callout variant="note" title="ownerOnly and shared data">
                        ownerOnly is a per-policy setting, not a per-entity setting. You can have some roles with ownerOnly and others without on the same entity. For example, a <code>member</code> can only see their own contacts while a <code>manager</code> can see all contacts in the same app.
                    </Callout>
                </section>

                {/* Evaluation Algorithm */}
                <section className="flex flex-col gap-4" id="evaluation">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Policy Evaluation Algorithm</h2>
                    <p className="text-muted-foreground leading-7">
                        The runtime evaluates RBAC on every authenticated request to the Data API. The algorithm runs entirely in memory using data loaded at boot and refreshed after each deployment.
                    </p>

                    <div className="flex flex-col gap-0">
                        {[
                            {
                                step: 1,
                                title: "Extract user identity",
                                detail: "Parse the Bearer token from the Authorization header. Validate the JWT signature and expiry. Extract the user_id claim. If the token is missing, invalid, or expired, return 401 Unauthorized immediately.",
                            },
                            {
                                step: 2,
                                title: "Load role assignments",
                                detail: "Query rootcx_system.rbac_assignments for all role names assigned to this user_id. The result is a set of direct role names (e.g. [\"member\", \"beta-tester\"]).",
                            },
                            {
                                step: 3,
                                title: "Resolve role hierarchy",
                                detail: "For each directly assigned role, perform a breadth-first traversal of the inheritance graph built from the manifest's roles array. Collect the full set of effective role names (direct + all ancestors).",
                            },
                            {
                                step: 4,
                                title: "Collect matching policies",
                                detail: "Scan the policy list for entries where role is in the effective role set AND entity matches either the requested entity name or \"*\" (wildcard). Collect all matching policies.",
                            },
                            {
                                step: 5,
                                title: "Compute permitted actions",
                                detail: "Take the union of all actions arrays across matching policies. This produces the complete set of actions the user is permitted to perform on this entity.",
                            },
                            {
                                step: 6,
                                title: "Check requested action",
                                detail: "If the HTTP request's action (e.g. \"delete\" for a DELETE request) is in the permitted actions set, proceed. Otherwise, return 403 Forbidden.",
                            },
                            {
                                step: 7,
                                title: "Apply ownership filter",
                                detail: "If any matching policy has ownerOnly: true AND the request is a read/update/delete, append the WHERE owner_id = $user_id predicate to the SQL query. Then execute the query.",
                            },
                        ].map(({ step, title, detail }) => (
                            <div key={step} className="flex gap-4 relative">
                                <div className="flex flex-col items-center">
                                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-border bg-[#0d0d0d] text-xs font-semibold text-foreground z-10">
                                        {step}
                                    </div>
                                    {step < 7 && <div className="w-px flex-1 bg-border/50 my-1" />}
                                </div>
                                <div className={`flex flex-col gap-1 pb-6 ${step === 7 ? "pb-0" : ""}`}>
                                    <p className="text-sm font-semibold text-foreground pt-1">{title}</p>
                                    <p className="text-sm text-muted-foreground leading-6">{detail}</p>
                                </div>
                            </div>
                        ))}
                    </div>

                    <Callout variant="tip" title="ownerOnly is an OR condition across policies">
                        If a user has two policies on the same entity — one with <code>ownerOnly: true</code> and one with <code>ownerOnly: false</code> — the ownership filter is NOT applied, because at least one policy grants unrestricted access. The filter only applies when all matching policies for the action have <code>ownerOnly: true</code>.
                    </Callout>
                </section>

                {/* RBAC Routes */}
                <section className="flex flex-col gap-4" id="rbac-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">RBAC API Routes</h2>
                    <p className="text-muted-foreground leading-7">
                        The runtime exposes a set of RBAC management endpoints under <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">/api/v1/rbac</code>. These routes are used to manage role assignments at runtime. They require authentication and typically require admin-level access.
                    </p>

                    <div className="flex flex-col gap-4">

                        <div className="rounded-lg border border-border overflow-hidden">
                            <div className="flex items-center gap-3 bg-[#0d0d0d] px-4 py-3 border-b border-border">
                                <span className="rounded bg-green-500/10 px-2 py-0.5 font-mono text-xs text-green-400">POST</span>
                                <code className="font-mono text-sm text-foreground">/api/v1/rbac/assign</code>
                            </div>
                            <div className="p-4">
                                <p className="text-sm text-muted-foreground mb-3">Assign a role to a user. Creates a record in <code className="font-mono text-xs">rootcx_system.rbac_assignments</code>. The assignment takes effect immediately on the next request from that user.</p>
                                <CodeBlock language="json" filename="Request body" code={`{
  "user_id": "3f7a1b2c-4d5e-6f7a-8b9c-0d1e2f3a4b5c",
  "role": "manager"
}`} />
                            </div>
                        </div>

                        <div className="rounded-lg border border-border overflow-hidden">
                            <div className="flex items-center gap-3 bg-[#0d0d0d] px-4 py-3 border-b border-border">
                                <span className="rounded bg-red-500/10 px-2 py-0.5 font-mono text-xs text-red-400">POST</span>
                                <code className="font-mono text-sm text-foreground">/api/v1/rbac/revoke</code>
                            </div>
                            <div className="p-4">
                                <p className="text-sm text-muted-foreground mb-3">Revoke a role from a user. Removes the assignment from <code className="font-mono text-xs">rootcx_system.rbac_assignments</code>. Effective on the next request.</p>
                                <CodeBlock language="json" filename="Request body" code={`{
  "user_id": "3f7a1b2c-4d5e-6f7a-8b9c-0d1e2f3a4b5c",
  "role": "manager"
}`} />
                            </div>
                        </div>

                        <div className="rounded-lg border border-border overflow-hidden">
                            <div className="flex items-center gap-3 bg-[#0d0d0d] px-4 py-3 border-b border-border">
                                <span className="rounded bg-blue-500/10 px-2 py-0.5 font-mono text-xs text-blue-400">GET</span>
                                <code className="font-mono text-sm text-foreground">/api/v1/rbac/assignments/:user_id</code>
                            </div>
                            <div className="p-4">
                                <p className="text-sm text-muted-foreground mb-3">List all roles directly assigned to a user. Returns an array of role names.</p>
                                <CodeBlock language="json" filename="Response" code={`{
  "user_id": "3f7a1b2c-...",
  "roles": ["member", "beta-tester"]
}`} />
                            </div>
                        </div>

                        <div className="rounded-lg border border-border overflow-hidden">
                            <div className="flex items-center gap-3 bg-[#0d0d0d] px-4 py-3 border-b border-border">
                                <span className="rounded bg-blue-500/10 px-2 py-0.5 font-mono text-xs text-blue-400">GET</span>
                                <code className="font-mono text-sm text-foreground">/api/v1/rbac/permissions/:user_id</code>
                            </div>
                            <div className="p-4">
                                <p className="text-sm text-muted-foreground mb-3">Returns the effective permissions for a user — the full resolved set of entity-action pairs after walking the inheritance graph. Useful for debugging access issues and building permission-aware UIs.</p>
                                <CodeBlock language="json" filename="Response" code={`{
  "user_id": "3f7a1b2c-...",
  "effective_roles": ["member", "beta-tester"],
  "permissions": {
    "contacts":  ["create", "read", "update"],
    "deals":     ["create", "read", "update"],
    "reports":   ["read"]
  },
  "owner_only_entities": ["contacts", "deals"]
}`} />
                            </div>
                        </div>

                    </div>
                </section>

                {/* Complete Example */}
                <section className="flex flex-col gap-4" id="complete-example">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Complete Example</h2>
                    <p className="text-muted-foreground leading-7">
                        The following permissions block defines a four-tier RBAC model for a project management application with fine-grained entity access and an auditor role with read-only access across the board.
                    </p>

                    <CodeBlock language="json" filename="manifest.json — permissions block" code={`"permissions": {
  "defaultRole": "member",

  "roles": [
    { "name": "admin" },
    { "name": "project-manager", "inherits": ["member"] },
    { "name": "member" },
    { "name": "auditor" }
  ],

  "policies": [
    {
      "role": "admin",
      "entity": "*",
      "actions": ["create", "read", "update", "delete"]
    },

    {
      "role": "project-manager",
      "entity": "projects",
      "actions": ["create", "read", "update", "delete"]
    },
    {
      "role": "project-manager",
      "entity": "tasks",
      "actions": ["create", "read", "update", "delete"]
    },
    {
      "role": "project-manager",
      "entity": "time_entries",
      "actions": ["read", "update", "delete"]
    },

    {
      "role": "member",
      "entity": "projects",
      "actions": ["read"]
    },
    {
      "role": "member",
      "entity": "tasks",
      "actions": ["create", "read", "update"],
      "ownerOnly": true
    },
    {
      "role": "member",
      "entity": "time_entries",
      "actions": ["create", "read", "update", "delete"],
      "ownerOnly": true
    },

    {
      "role": "auditor",
      "entity": "*",
      "actions": ["read"]
    }
  ]
}`} />

                    <p className="text-muted-foreground leading-7">
                        With this configuration:
                    </p>
                    <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-disc list-inside">
                        <li><strong className="text-foreground font-medium">admin</strong> — full CRUD on all entities, no ownership restrictions.</li>
                        <li><strong className="text-foreground font-medium">project-manager</strong> — full CRUD on projects and tasks, can view/edit/delete any time entry. Inherits member's read on projects and ownerOnly task/time-entry permissions (but the explicit wider policies supersede them).</li>
                        <li><strong className="text-foreground font-medium">member</strong> — read-only on projects, owner-only create/read/update on tasks, full CRUD on their own time entries.</li>
                        <li><strong className="text-foreground font-medium">auditor</strong> — read-only across all entities, no ownership filter (can read all records).</li>
                    </ul>
                </section>

                {/* Database Schema */}
                <section className="flex flex-col gap-4" id="database-schema">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Database Schema</h2>
                    <p className="text-muted-foreground leading-7">
                        The RBAC system uses three tables in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system</code> schema. These are created automatically during the boot sequence and managed exclusively by the runtime.
                    </p>

                    <CodeBlock language="sql" filename="rootcx_system RBAC tables" code={`-- Roles defined by the manifest
CREATE TABLE rootcx_system.rbac_roles (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id     TEXT        NOT NULL,
    name       TEXT        NOT NULL,
    inherits   TEXT[]      NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (app_id, name)
);

-- Runtime role assignments (user → role)
CREATE TABLE rootcx_system.rbac_assignments (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id     TEXT        NOT NULL,
    user_id    UUID        NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
    role       TEXT        NOT NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    assigned_by UUID       REFERENCES rootcx_system.users(id),
    UNIQUE (app_id, user_id, role)
);

-- Access control policies from the manifest
CREATE TABLE rootcx_system.rbac_policies (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id     TEXT        NOT NULL,
    role       TEXT        NOT NULL,
    entity     TEXT        NOT NULL,  -- entity name or '*'
    actions    TEXT[]      NOT NULL,
    owner_only BOOLEAN     NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (app_id, role, entity)
);`} />

                    <Callout variant="info" title="Policy table is rewritten on deploy">
                        Every time a new manifest is deployed, the runtime deletes all existing rows in <code>rootcx_system.rbac_policies</code> and <code>rootcx_system.rbac_roles</code> for the app and reinserts them from the new manifest. Role <em>assignments</em> in <code>rootcx_system.rbac_assignments</code> are preserved across deployments — existing user-role mappings are not affected by manifest updates.
                    </Callout>
                </section>

                {/* Related */}
                <section className="flex flex-col gap-4" id="related">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Related Pages</h2>
                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        {[
                            { href: "/concepts/manifest", title: "App Manifest", desc: "The full manifest structure where roles and policies are declared." },
                            { href: "/modules/rbac", title: "RBAC Module", desc: "Full API reference for role assignment, revocation, and permission queries." },
                            { href: "/modules/authentication", title: "Authentication", desc: "How users register and obtain JWTs used in RBAC evaluation." },
                            { href: "/modules/audit", title: "Audit Logs", desc: "Every RBAC operation is recorded in the immutable audit log." },
                        ].map(({ href, title, desc }) => (
                            <Link key={href} href={href} className="group rounded-lg border border-border bg-white/[0.02] p-4 transition-colors hover:border-border/80 hover:bg-white/[0.04]">
                                <div className="flex items-center justify-between">
                                    <p className="text-sm font-medium text-foreground">{title}</p>
                                    <ChevronRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
                                </div>
                                <p className="mt-1 text-xs text-muted-foreground leading-relaxed">{desc}</p>
                            </Link>
                        ))}
                    </div>
                </section>

                <PageNav href="/concepts/permissions" />
            </div>
        </DocsLayout>
    );
}
