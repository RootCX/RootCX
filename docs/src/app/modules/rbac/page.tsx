import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const tocItems = [
  { id: "overview", title: "Overview" },
  { id: "roles", title: "Roles" },
  { id: "policies", title: "Policies" },
  { id: "actions", title: "Actions" },
  { id: "ownership", title: "Ownership" },
  { id: "role-inheritance", title: "Role Inheritance" },
  { id: "default-role", title: "Default Role" },
  { id: "rbac-routes", title: "RBAC API Routes" },
  { id: "policy-evaluation", title: "Policy Evaluation" },
  { id: "database-schema", title: "Database Schema" },
];

export default function RbacPage() {
  return (
    <DocsLayout toc={tocItems}>
      <div className="flex flex-col gap-10">

          {/* Breadcrumb */}
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
            <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
            <ChevronRight className="h-3 w-3" />
            <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
            <ChevronRight className="h-3 w-3" />
            <span className="text-foreground">RBAC</span>
          </div>

          {/* Title */}
          <div className="flex flex-col gap-3">
            <h1 className="text-4xl font-bold tracking-tight">RBAC</h1>
            <p className="text-lg text-muted-foreground leading-7">
              Fine-grained role-based access control with policy inheritance and row-level ownership filters.
            </p>
          </div>

          {/* Overview */}
          <section className="flex flex-col gap-4" id="overview">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
            <p className="text-muted-foreground leading-7">
              The RBAC extension enforces <strong className="text-foreground font-medium">who can do what on which entity</strong> across
              every data route in your application. When RBAC is installed, every incoming request to a data endpoint is
              evaluated against the policy table before the query is executed. Unauthorized requests receive a{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">403 Forbidden</code> response immediately,
              and no database interaction occurs.
            </p>
            <p className="text-muted-foreground leading-7">
              RBAC is declared entirely inside your application manifest. You define roles, attach policies to those roles,
              and the runtime enforces them automatically. There is no middleware to wire up and no guards to write — the
              enforcement layer lives inside the Core runtime itself.
            </p>
            <Callout variant="info">
              RBAC requires the <strong className="text-foreground font-medium">authentication</strong> module to be installed first.
              Every policy check begins by decoding the JWT from the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Authorization</code> header to identify
              the requesting user.
            </Callout>
            <p className="text-muted-foreground leading-7">
              A minimal manifest with RBAC enabled looks like this:
            </p>
            <CodeBlock language="json" code={`{
  "id": "my-app",
  "name": "My Application",
  "modules": ["auth", "data", "rbac"],
  "entities": [
    {
      "name": "posts",
      "fields": [
        { "name": "title",   "type": "text" },
        { "name": "content", "type": "text" }
      ]
    }
  ],
  "permissions": {
    "roles": [
      {
        "name": "admin",
        "description": "Full access to all entities",
        "policies": [
          { "entity": "posts", "actions": ["create", "read", "update", "delete"] }
        ]
      },
      {
        "name": "viewer",
        "description": "Read-only access",
        "policies": [
          { "entity": "posts", "actions": ["read"] }
        ]
      }
    ],
    "defaultRole": "viewer"
  }
}`} />
          </section>

          {/* Roles */}
          <section className="flex flex-col gap-4" id="roles">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Roles</h2>
            <p className="text-muted-foreground leading-7">
              A <strong className="text-foreground font-medium">role</strong> is a named group of permissions scoped to a single
              application. Roles are defined in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permissions.roles</code> array
              of the manifest. Users can be assigned one or more roles, and their effective permission set is the union of
              all policies attached to all of their roles (including inherited ones).
            </p>
            <PropertiesTable
              properties={[
                {
                  name: "name",
                  type: "string",
                  required: true,
                  description: "Unique identifier for the role within this application. Used when assigning or revoking roles from users.",
                },
                {
                  name: "description",
                  type: "string",
                  required: false,
                  description: "Human-readable description of the role's purpose. Stored in the database and returned by the roles API.",
                },
                {
                  name: "inherits",
                  type: "string[]",
                  required: false,
                  description: "List of role names whose permissions this role inherits. Resolution is recursive — inherited roles may themselves inherit from other roles.",
                },
                {
                  name: "policies",
                  type: "Policy[]",
                  required: false,
                  description: "List of permission policies attached directly to this role. See the Policies section for the policy object shape.",
                },
              ]}
            />
            <p className="text-muted-foreground leading-7">
              A fully populated roles definition with three roles and inheritance:
            </p>
            <CodeBlock language="json" code={`{
  "permissions": {
    "defaultRole": "viewer",
    "roles": [
      {
        "name": "viewer",
        "description": "Can read all public content",
        "policies": [
          { "entity": "posts",    "actions": ["read"] },
          { "entity": "comments", "actions": ["read"] }
        ]
      },
      {
        "name": "editor",
        "description": "Can create and manage own content",
        "inherits": ["viewer"],
        "policies": [
          { "entity": "posts",    "actions": ["create", "update", "delete"], "ownership": true },
          { "entity": "comments", "actions": ["create", "update", "delete"], "ownership": true }
        ]
      },
      {
        "name": "admin",
        "description": "Unrestricted access to all entities",
        "inherits": ["editor"],
        "policies": [
          { "entity": "posts",    "actions": ["create", "read", "update", "delete"] },
          { "entity": "comments", "actions": ["create", "read", "update", "delete"] },
          { "entity": "users",    "actions": ["read", "update", "delete"] }
        ]
      }
    ]
  }
}`} />
          </section>

          {/* Policies */}
          <section className="flex flex-col gap-4" id="policies">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Policies</h2>
            <p className="text-muted-foreground leading-7">
              A <strong className="text-foreground font-medium">policy</strong> links a role to an entity and specifies which
              actions that role may perform on that entity. Policies are embedded inside the role definition under the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">policies</code> key.
            </p>
            <PropertiesTable
              properties={[
                {
                  name: "entity",
                  type: "string",
                  required: true,
                  description: "The entity name as declared in the manifest's entities array. Must be an exact match.",
                },
                {
                  name: "actions",
                  type: "string[]",
                  required: true,
                  description: 'Array of allowed actions. Valid values: "create", "read", "update", "delete".',
                },
                {
                  name: "ownership",
                  type: "boolean",
                  required: false,
                  description: "When true, restricts write and delete operations to rows the user owns (owner_id = current user). Read operations are also filtered to owned rows only. Defaults to false.",
                },
              ]}
            />
            <p className="text-muted-foreground leading-7">
              Policies are stored in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rbac_policies</code> table
              and loaded into an in-memory cache when the app is installed. The cache is invalidated and rebuilt whenever
              the app is reinstalled. This means policy enforcement adds zero database round-trips to the hot path.
            </p>
          </section>

          {/* Actions */}
          <section className="flex flex-col gap-4" id="actions">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Actions</h2>
            <p className="text-muted-foreground leading-7">
              Actions map directly to HTTP methods on the data routes. Each action corresponds to one or more request
              types that the policy gate checks before allowing a request to proceed.
            </p>
            <div className="overflow-x-auto rounded-lg border border-border">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border bg-muted/30">
                    <th className="text-left px-4 py-3 font-medium text-foreground">Action</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">HTTP Method</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Route Pattern</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Description</th>
                  </tr>
                </thead>
                <tbody>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">create</code></td>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">POST</code></td>
                    <td className="px-4 py-3 text-muted-foreground">/api/v1/apps/:appId/:entity</td>
                    <td className="px-4 py-3 text-muted-foreground">Insert a new row into the entity table</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">read</code></td>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET</code></td>
                    <td className="px-4 py-3 text-muted-foreground">/api/v1/apps/:appId/:entity[/:id]</td>
                    <td className="px-4 py-3 text-muted-foreground">Fetch one or many rows from the entity table</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">update</code></td>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">PATCH</code></td>
                    <td className="px-4 py-3 text-muted-foreground">/api/v1/apps/:appId/:entity/:id</td>
                    <td className="px-4 py-3 text-muted-foreground">Partially update an existing row by primary key</td>
                  </tr>
                  <tr>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">delete</code></td>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">DELETE</code></td>
                    <td className="px-4 py-3 text-muted-foreground">/api/v1/apps/:appId/:entity/:id</td>
                    <td className="px-4 py-3 text-muted-foreground">Remove a row by primary key</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p className="text-muted-foreground leading-7">
              If a user has no policy entry for a given entity, <strong className="text-foreground font-medium">all actions are denied</strong> on
              that entity. There is no implicit allow — every permitted action must be explicitly declared.
            </p>
          </section>

          {/* Ownership */}
          <section className="flex flex-col gap-4" id="ownership">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Ownership</h2>
            <p className="text-muted-foreground leading-7">
              Setting <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ownership: true</code> on a policy
              activates row-level ownership filtering for that role and entity combination. When ownership is enabled:
            </p>
            <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-none pl-0">
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>An <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id</code> column is automatically added to the entity's database table if it does not already exist. The column type is <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">UUID</code> and references the authenticated user's ID.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>On <strong className="text-foreground font-medium">create</strong>, the runtime automatically injects <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id = current_user_id</code> into the INSERT statement. The client cannot override this value.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>On <strong className="text-foreground font-medium">read</strong>, a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">WHERE owner_id = current_user_id</code> filter is appended to the query. Users see only their own rows.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>On <strong className="text-foreground font-medium">update</strong> and <strong className="text-foreground font-medium">delete</strong>, the same <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">WHERE owner_id = current_user_id</code> condition is added. Attempts to mutate rows owned by another user return <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">404 Not Found</code> rather than <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">403</code> to avoid leaking record existence.</span>
              </li>
            </ul>
            <Callout variant="info">
              Ownership filtering is computed at the role level. If a user has two roles — one with ownership and one
              without — for the same entity, the <strong className="text-foreground font-medium">non-ownership</strong> policy wins and the user
              has unrestricted access. Ownership is only applied when every effective policy for that entity has
              ownership enabled.
            </Callout>
            <CodeBlock language="json" code={`// Editor role: can only read/write their own posts
{
  "name": "editor",
  "policies": [
    {
      "entity": "posts",
      "actions": ["create", "read", "update", "delete"],
      "ownership": true
    }
  ]
}

// Admin role: can read/write ALL posts regardless of owner
{
  "name": "admin",
  "policies": [
    {
      "entity": "posts",
      "actions": ["create", "read", "update", "delete"]
    }
  ]
}`} />
          </section>

          {/* Role Inheritance */}
          <section className="flex flex-col gap-4" id="role-inheritance">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Role Inheritance</h2>
            <p className="text-muted-foreground leading-7">
              A role can inherit the permissions of one or more other roles by listing their names in the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">inherits</code> array.
              Inheritance is <strong className="text-foreground font-medium">recursive</strong> — if role A inherits from role B, and role B
              inherits from role C, then a user assigned role A effectively has all permissions from A, B, and C.
            </p>
            <p className="text-muted-foreground leading-7">
              The runtime resolves the full inheritance graph at policy evaluation time using a depth-first traversal
              with cycle detection. Circular inheritance references are silently broken to prevent infinite loops.
            </p>
            <CodeBlock language="json" code={`// Inheritance chain: admin → editor → viewer
{
  "roles": [
    {
      "name": "viewer",
      "policies": [
        { "entity": "posts", "actions": ["read"] }
      ]
    },
    {
      "name": "editor",
      "inherits": ["viewer"],
      "policies": [
        { "entity": "posts", "actions": ["create", "update"], "ownership": true }
      ]
    },
    {
      "name": "admin",
      "inherits": ["editor"],
      "policies": [
        { "entity": "posts",  "actions": ["create", "read", "update", "delete"] },
        { "entity": "users",  "actions": ["read", "update", "delete"] }
      ]
    }
  ]
}

// Effective permissions for a user assigned "admin":
// posts → create, read, update, delete (no ownership filter — admin policy overrides)
// users → read, update, delete`} />
            <p className="text-muted-foreground leading-7">
              When merging policies from inherited roles, the runtime takes the <strong className="text-foreground font-medium">union of all
              allowed actions</strong>. If any policy for the same entity omits ownership filtering, the ownership constraint
              is lifted for that role.
            </p>
          </section>

          {/* Default Role */}
          <section className="flex flex-col gap-4" id="default-role">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Default Role</h2>
            <p className="text-muted-foreground leading-7">
              The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">defaultRole</code> field in the
              permissions block names the role that is automatically assigned to every new user when they register via the
              authentication module. This ensures newly registered users always have a baseline set of permissions
              without requiring a manual role assignment step.
            </p>
            <CodeBlock language="json" code={`{
  "permissions": {
    "defaultRole": "viewer",
    "roles": [...]
  }
}`} />
            <p className="text-muted-foreground leading-7">
              The default role is assigned by inserting a row into{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rbac_assignments</code> at the moment the
              user record is created. You can subsequently promote or demote users using the assign/revoke endpoints.
            </p>
            <p className="text-muted-foreground leading-7">
              If <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">defaultRole</code> is omitted, new users
              receive no role and will be denied access to all RBAC-protected routes until a role is explicitly assigned.
            </p>
          </section>

          {/* RBAC API Routes */}
          <section className="flex flex-col gap-4" id="rbac-routes">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">RBAC API Routes</h2>
            <p className="text-muted-foreground leading-7">
              The runtime exposes a set of management endpoints for inspecting and mutating role assignments. All routes
              require an authenticated request (valid JWT in the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Authorization: Bearer</code> header). Role
              assignment and revocation require the caller to have admin-level access.
            </p>

            <h3 className="text-lg font-semibold text-foreground mt-2">List All Roles</h3>
            <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/roles

curl https://your-runtime.com/api/v1/apps/my-app/roles \\
  -H "Authorization: Bearer <token>"

# Response
[
  {
    "name": "admin",
    "description": "Unrestricted access to all entities",
    "inherits": ["editor"]
  },
  {
    "name": "editor",
    "description": "Can create and manage own content",
    "inherits": ["viewer"]
  },
  {
    "name": "viewer",
    "description": "Read-only access"
  }
]`} />

            <h3 className="text-lg font-semibold text-foreground mt-2">List Role Assignments</h3>
            <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/roles/assignments

curl https://your-runtime.com/api/v1/apps/my-app/roles/assignments \\
  -H "Authorization: Bearer <token>"

# Response
[
  { "user_id": "user_abc123", "role": "admin" },
  { "user_id": "user_def456", "role": "editor" },
  { "user_id": "user_ghi789", "role": "viewer" }
]`} />

            <h3 className="text-lg font-semibold text-foreground mt-2">Assign a Role</h3>
            <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/roles/assign
Content-Type: application/json

{
  "userId": "user_abc123",
  "role":   "admin"
}

curl -X POST https://your-runtime.com/api/v1/apps/my-app/roles/assign \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/json" \\
  -d '{"userId": "user_abc123", "role": "admin"}'

# Response
{ "ok": true }`} />

            <h3 className="text-lg font-semibold text-foreground mt-2">Revoke a Role</h3>
            <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/roles/revoke
Content-Type: application/json

{
  "userId": "user_abc123",
  "role":   "admin"
}

curl -X POST https://your-runtime.com/api/v1/apps/my-app/roles/revoke \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/json" \\
  -d '{"userId": "user_abc123", "role": "admin"}'

# Response
{ "ok": true }`} />

            <h3 className="text-lg font-semibold text-foreground mt-2">Get Current User Permissions</h3>
            <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/permissions

curl https://your-runtime.com/api/v1/apps/my-app/permissions \\
  -H "Authorization: Bearer <token>"

# Response
{
  "userId": "user_abc123",
  "roles":  ["admin"],
  "permissions": {
    "posts":    { "actions": ["create","read","update","delete"], "ownership": false },
    "comments": { "actions": ["create","read","update","delete"], "ownership": false },
    "users":    { "actions": ["read","update","delete"],          "ownership": false }
  }
}`} />

            <h3 className="text-lg font-semibold text-foreground mt-2">Get Specific User Permissions</h3>
            <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/permissions/{userId}

curl https://your-runtime.com/api/v1/apps/my-app/permissions/user_def456 \\
  -H "Authorization: Bearer <token>"

# Response
{
  "userId": "user_def456",
  "roles":  ["editor"],
  "permissions": {
    "posts":    { "actions": ["create","read","update","delete"], "ownership": true },
    "comments": { "actions": ["create","read","update","delete"], "ownership": true }
  }
}`} />
          </section>

          {/* Policy Evaluation */}
          <section className="flex flex-col gap-4" id="policy-evaluation">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Policy Evaluation</h2>
            <p className="text-muted-foreground leading-7">
              Every request to a data route passes through the RBAC policy evaluator. The evaluation algorithm runs
              entirely in memory using the cached policy set and resolves in constant time relative to the number of
              policies. The steps are:
            </p>
            <div className="flex flex-col gap-3">
              {[
                {
                  step: "1",
                  title: "Extract JWT",
                  body: "Decode the Bearer token from the Authorization header. Verify the signature against the app's secret. Extract the user_id claim. Reject with 401 if missing or invalid.",
                },
                {
                  step: "2",
                  title: "Load User Roles",
                  body: "Query rbac_assignments WHERE user_id = :userId AND app_id = :appId. This single indexed lookup returns the list of role names the user holds.",
                },
                {
                  step: "3",
                  title: "Resolve Role Hierarchy",
                  body: "For each role, recursively follow the inherits[] graph to collect the full set of effective roles. Deduplicate using a visited set to handle diamonds and prevent infinite loops.",
                },
                {
                  step: "4",
                  title: "Collect All Policies",
                  body: "From the in-memory policy cache, retrieve all policy entries matching (app_id, role, entity) for the full set of effective roles.",
                },
                {
                  step: "5",
                  title: "Gather Allowed Actions",
                  body: "Take the union of all actions arrays across all collected policies. If the requested HTTP action is not in this union, return 403 Forbidden immediately.",
                },
                {
                  step: "6",
                  title: "Compute Ownership Flag",
                  body: "The ownership filter is applied only if every policy for this entity across all effective roles has ownership: true. If any policy omits ownership, the filter is disabled.",
                },
                {
                  step: "7",
                  title: "Apply Filters",
                  body: "Pass the resolved (actions, ownership) tuple to the data layer. If ownership is true, inject WHERE owner_id = :userId into the generated SQL. Execute the query and return the result.",
                },
              ].map(({ step, title, body }) => (
                <div key={step} className="flex gap-4 rounded-lg border border-border p-4">
                  <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-bold text-foreground">
                    {step}
                  </div>
                  <div className="flex flex-col gap-1">
                    <span className="text-sm font-semibold text-foreground">{title}</span>
                    <span className="text-sm text-muted-foreground leading-6">{body}</span>
                  </div>
                </div>
              ))}
            </div>
            <Callout variant="info">
              Steps 1 through 5 run entirely against the <strong className="text-foreground font-medium">in-memory policy cache</strong>. Only
              step 2 (load user roles) touches the database. The overall overhead of RBAC enforcement is approximately
              one indexed query per request.
            </Callout>
          </section>

          {/* Database Schema */}
          <section className="flex flex-col gap-4" id="database-schema">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Database Schema</h2>
            <p className="text-muted-foreground leading-7">
              Installing the RBAC module creates three tables in your application's PostgreSQL database. These tables
              are managed by the runtime and should not be modified directly.
            </p>
            <CodeBlock language="sql" code={`-- Named roles defined in the manifest
CREATE TABLE rbac_roles (
  id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  app_id      TEXT        NOT NULL,
  name        TEXT        NOT NULL,
  description TEXT,
  inherits    TEXT[]      NOT NULL DEFAULT '{}',
  created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (app_id, name)
);

-- User-to-role assignments
CREATE TABLE rbac_assignments (
  id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id    UUID        NOT NULL,
  app_id     TEXT        NOT NULL,
  role       TEXT        NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE (user_id, app_id, role)
);

-- Per-role, per-entity permission policies
CREATE TABLE rbac_policies (
  id        UUID    PRIMARY KEY DEFAULT gen_random_uuid(),
  app_id    TEXT    NOT NULL,
  role      TEXT    NOT NULL,
  entity    TEXT    NOT NULL,
  actions   TEXT[]  NOT NULL DEFAULT '{}',
  ownership BOOLEAN NOT NULL DEFAULT FALSE,
  UNIQUE (app_id, role, entity)
);

CREATE INDEX idx_rbac_assignments_user ON rbac_assignments (user_id, app_id);
CREATE INDEX idx_rbac_policies_role    ON rbac_policies    (app_id, role);`} />
            <p className="text-muted-foreground leading-7">
              On every app install (or reinstall), the runtime syncs the manifest's permissions block with these
              tables — inserting new roles, updating changed policies, and removing stale entries. The in-memory policy
              cache is rebuilt from the database after each sync completes.
            </p>
          </section>

        <PageNav href="/modules/rbac" />
      </div>
    </DocsLayout>
  );
}
