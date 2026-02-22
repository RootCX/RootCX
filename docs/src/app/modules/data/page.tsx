import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
  { id: "outcomes", title: "Key Outcomes" },
  { id: "overview", title: "Overview" },
  { id: "list-records", title: "List Records" },
  { id: "create-record", title: "Create Record" },
  { id: "get-record", title: "Get Record" },
  { id: "update-record", title: "Update Record" },
  { id: "delete-record", title: "Delete Record" },
  { id: "type-binding", title: "Type Binding" },
  { id: "error-handling", title: "Error Handling" },
];

export default function DataManagementPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        {/* Breadcrumb */}
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Data Management</span>
        </div>

        {/* Title */}
        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Data Management</h1>
          <p className="text-lg text-muted-foreground leading-7">
            Automatic CRUD REST APIs generated from your application manifest. Every entity declared in your
            schema gets a fully functional, type-safe, ownership-aware HTTP API with zero additional configuration.
          </p>
        </div>

        {/* Outcomes */}
        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "No boilerplate code: Eliminate the need to write repetitive controller, service, and repository code for CRUD operations.",
              "AI Integration ready: RootCX automatically generates native MCP tools for your data endpoints, allowing external AI agents to securely interact with your internal software instantly.",
              "Ownership aware: Every generated API is protected by JWT authentication and automatically scopes data access to the record owner or appropriate roles."
            ].map((item, i) => (
              <li key={i} className="flex items-start gap-2">
                <span className="mt-2 flex-shrink-0 w-1.5 h-1.5 rounded-full bg-primary/60" />
                <span dangerouslySetInnerHTML={{ __html: item.replace(/^([^:]+:)/, '<strong>$1</strong>') }} />
              </li>
            ))}
          </ul>
        </section>

        {/* Overview */}
        <section className="flex flex-col gap-4" id="overview">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
          <p className="text-muted-foreground leading-7">
            When you define an entity in your RootCX manifest, Core automatically provisions a PostgreSQL
            table and exposes a complete set of CRUD endpoints for that entity. You do not write any controller
            code — Core introspects the manifest schema and generates type-aware SQL bindings, ownership
            enforcement, and validation at startup.
          </p>
          <p className="text-muted-foreground leading-7">
            In addition to the HTTP REST API, RootCX <strong>automatically generates native tools</strong> for
            these endpoints and exposes them directly to built-in AI workflows. It also spins up an{" "}
            <strong>MCP server</strong> (Model Context Protocol) exposing the same tools, allowing any external
            AI agent to securely interact with the internal software you have built.
          </p>
          <p className="text-muted-foreground leading-7">
            Every request to a data endpoint is authenticated via the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Authorization: Bearer</code>{" "}
            header. The authenticated user&#39;s ID is extracted from the JWT and used to scope all read and write
            operations to records the user owns, unless an RBAC role grants broader access.
          </p>

          <h3 className="text-lg font-semibold text-foreground mt-2">Base URL Pattern</h3>
          <p className="text-muted-foreground leading-7">
            All data endpoints follow a consistent URL structure:
          </p>
          <CodeBlock language="text" code={`/api/v1/apps/{appId}/collections/{entity}`} />
          <p className="text-muted-foreground leading-7">
            Where <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">appId</code> is
            your application&#39;s unique identifier and{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">entity</code> is
            the snake_case name of your manifest entity (e.g.{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">posts</code>,{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">orders</code>,{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">product_reviews</code>).
          </p>

          <h3 className="text-lg font-semibold text-foreground mt-2">System Fields</h3>
          <p className="text-muted-foreground leading-7">
            Every entity table automatically includes the following system-managed columns. These fields cannot
            be set by the client on create or update — Core enforces their values:
          </p>
          <PropertiesTable
            properties={[
              {
                name: "id",
                type: "TEXT",
                description: "UUID v4, generated by Core on record creation. Globally unique.",
              },
              {
                name: "owner_id",
                type: "TEXT",
                description: "The user ID extracted from the JWT of the authenticated user who created the record.",
              },
              {
                name: "created_at",
                type: "TIMESTAMPTZ",
                description: "ISO 8601 timestamp set at insertion time. Never modified.",
              },
              {
                name: "updated_at",
                type: "TIMESTAMPTZ",
                description: "ISO 8601 timestamp updated automatically on every PATCH request.",
              },
            ]}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Example Manifest Entity</h3>
          <CodeBlock
            language="json"
            code={`{
  "appId": "my-app",
  "name": "My Application",
  "version": "1.0.0",
  "permissions": {
    "defaultRole": "member",
    "roles": [{ "name": "member" }],
    "policies": []
  },
  "dataContract": [
    {
      "entityName": "posts",
      "fields": [
        { "name": "title", "type": "text", "required": true },
        { "name": "body", "type": "text" },
        { "name": "published", "type": "boolean", "defaultValue": "false" },
        { "name": "tags", "type": "json" },
        { "name": "published_at", "type": "date" }
      ]
    }
  ]
}`}
          />
        </section>

        {/* List Records */}
        <section className="flex flex-col gap-4" id="list-records">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">List Records</h2>
          <p className="text-muted-foreground leading-7">
            Retrieves all records for the authenticated user within the specified entity collection. Results are
            scoped to the authenticated user&#39;s ownership unless an admin-level RBAC role is assigned.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/collections/{"{entity}"}
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl http://localhost:9100/api/v1/apps/my-app/collections/posts \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`[
  {
    "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
    "owner_id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
    "title": "Hello World",
    "body": "My first post.",
    "published": false,
    "tags": null,
    "published_at": null,
    "created_at": "2025-01-15T10:30:00.000Z",
    "updated_at": "2025-01-15T10:30:00.000Z"
  }
]`}
          />

          <Callout variant="info" title="Filtering and Pagination">
            The built-in list endpoint returns all owned records without cursor-based pagination or column
            filtering. For advanced querying — field filters, pagination, joins, or cross-user aggregations —
            expose a Backend function as an RPC endpoint and run arbitrary SQL there.
          </Callout>
        </section>

        {/* Create Record */}
        <section className="flex flex-col gap-4" id="create-record">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Create Record</h2>
          <p className="text-muted-foreground leading-7">
            Creates a new record in the entity collection. Core validates the request body against the
            manifest schema, injects all system fields, and persists the record. Returns the full created record
            with all system fields populated.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/collections/{"{entity}"}
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">Request Body</h3>
          <p className="text-muted-foreground leading-7">
            Send a JSON object containing only the user-defined fields from the manifest. System fields are
            always set by Core and silently ignored if included.
          </p>

          <PropertiesTable
            properties={[
              {
                name: "title",
                type: "string",
                required: true,
                description: "Required if the manifest field has required: true. A 400 is returned if absent.",
              },
              {
                name: "body",
                type: "string",
                required: false,
                description: "Optional text fields default to NULL unless a manifest default is defined.",
              },
              {
                name: "published",
                type: "boolean",
                required: false,
                description: "Boolean fields are bound as SQL BOOLEAN. Defaults to the manifest default value.",
              },
              {
                name: "tags",
                type: "object | array",
                required: false,
                description: "JSON fields accept any valid JSON value. Validated and stored as JSONB.",
              },
              {
                name: "published_at",
                type: "string (ISO 8601)",
                required: false,
                description: "Date/datetime fields are parsed and stored as TIMESTAMPTZ. Invalid dates return 400.",
              },
            ]}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X POST http://localhost:9100/api/v1/apps/my-app/collections/posts \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "title": "Hello World",
    "body": "My first post content.",
    "published": false,
    "tags": ["intro", "welcome"]
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 201 Created</h3>
          <CodeBlock
            language="json"
            code={`{
  "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
  "owner_id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "title": "Hello World",
  "body": "My first post content.",
  "published": false,
  "tags": ["intro", "welcome"],
  "published_at": null,
  "created_at": "2025-01-15T10:30:00.000Z",
  "updated_at": "2025-01-15T10:30:00.000Z"
}`}
          />
        </section>

        {/* Get Record */}
        <section className="flex flex-col gap-4" id="get-record">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Get Record</h2>
          <p className="text-muted-foreground leading-7">
            Retrieves a single record by its ID. Core performs both an existence check and an ownership
            check in a single query. If the record does not exist or belongs to a different user, a{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">404 Not Found</code> is
            returned — the same response is used in both cases to prevent ID enumeration attacks.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/collections/{"{entity}"}/{"{id}"}
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl http://localhost:9100/api/v1/apps/my-app/collections/posts/01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
  "owner_id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "title": "Hello World",
  "body": "My first post content.",
  "published": false,
  "tags": ["intro", "welcome"],
  "published_at": null,
  "created_at": "2025-01-15T10:30:00.000Z",
  "updated_at": "2025-01-15T10:30:00.000Z"
}`}
          />
        </section>

        {/* Update Record */}
        <section className="flex flex-col gap-4" id="update-record">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Update Record</h2>
          <p className="text-muted-foreground leading-7">
            Partially updates an existing record. Only the fields present in the request body are modified —
            omitted fields retain their current values. The{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">updated_at</code>{" "}
            timestamp is always set to the current UTC time. System fields{" "}
            (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">id</code>,{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id</code>,{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">created_at</code>)
            are immutable and silently stripped from the payload.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-yellow-400 bg-yellow-400/10 rounded px-2 py-0.5">PATCH</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/collections/{"{entity}"}/{"{id}"}
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X PATCH http://localhost:9100/api/v1/apps/my-app/collections/posts/01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "published": true,
    "published_at": "2025-01-15T12:00:00.000Z"
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
  "owner_id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "title": "Hello World",
  "body": "My first post content.",
  "published": true,
  "tags": ["intro", "welcome"],
  "published_at": "2025-01-15T12:00:00.000Z",
  "created_at": "2025-01-15T10:30:00.000Z",
  "updated_at": "2025-01-15T12:05:32.871Z"
}`}
          />
        </section>

        {/* Delete Record */}
        <section className="flex flex-col gap-4" id="delete-record">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Delete Record</h2>
          <p className="text-muted-foreground leading-7">
            Permanently deletes a record. Core verifies ownership before deletion. If the record does not
            exist or is owned by another user, a{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">404 Not Found</code> is returned.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-red-400 bg-red-400/10 rounded px-2 py-0.5">DELETE</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/collections/{"{entity}"}/{"{id}"}
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X DELETE http://localhost:9100/api/v1/apps/my-app/collections/posts/01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "message": "record '01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a' deleted"
}`}
          />

          <Callout variant="warning" title="Permanent Deletion">
            Records deleted via this endpoint are permanently removed. The audit log captures the deleted
            row&#39;s data if the Audit Logs module is enabled, but no soft-delete mechanism is built into the
            data module.
          </Callout>
        </section>

        {/* Type Binding */}
        <section className="flex flex-col gap-4" id="type-binding">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Type Binding</h2>
          <p className="text-muted-foreground leading-7">
            Core parses each field&#39;s value from the JSON request body and converts it to the appropriate
            SQL type before binding parameters. This prevents type coercion bugs and ensures the database
            receives well-typed values.
          </p>

          <PropertiesTable
            properties={[
              {
                name: "text",
                type: "SQL TEXT",
                description: "Bound as-is. Any JSON string value is accepted.",
              },
              {
                name: "integer",
                type: "SQL INTEGER",
                description: "Parsed with parseInt(). Non-integer values return a 400 validation error.",
              },
              {
                name: "float",
                type: "SQL DOUBLE PRECISION",
                description: "Parsed with parseFloat(). Non-numeric values return a 400 validation error.",
              },
              {
                name: "boolean",
                type: "SQL BOOLEAN",
                description: "Accepts JSON true/false. String values are not coerced — send actual JSON booleans.",
              },
              {
                name: "date / datetime",
                type: "SQL TIMESTAMPTZ",
                description: "Parsed via new Date(value). Invalid date strings return a 400 error. Always stored in UTC.",
              },
              {
                name: "json",
                type: "SQL JSONB",
                description: "Accepts any JSON-serializable value (object, array, string, number, null). Stored as JSONB.",
              },
              {
                name: "array",
                type: "SQL TEXT[]",
                description: "JSON arrays of strings are stored as PostgreSQL TEXT[]. Mixed-type arrays are rejected.",
              },
            ]}
          />
        </section>

        {/* Error Handling */}
        <section className="flex flex-col gap-4" id="error-handling">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Error Handling</h2>
          <p className="text-muted-foreground leading-7">
            All error responses share a consistent JSON structure. The{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">error</code> field
            is always a human-readable string. An optional{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">details</code> object
            provides structured context for validation errors.
          </p>

          <PropertiesTable
            properties={[
              {
                name: "400 Bad Request",
                type: "Validation Error",
                description: "A required field is missing, a field value cannot be coerced to its declared type, or the JSON body is malformed.",
              },
              {
                name: "401 Unauthorized",
                type: "Authentication Error",
                description: "No Authorization header was provided, or the Bearer token is missing, expired, or invalid.",
              },
              {
                name: "403 Forbidden",
                type: "Authorization Error",
                description: "The token is valid but the user's RBAC role does not permit the requested operation on this entity.",
              },
              {
                name: "404 Not Found",
                type: "Not Found",
                description: "The record does not exist, or exists but is owned by a different user. Both cases return identical 404 responses.",
              },
              {
                name: "500 Internal Server Error",
                type: "Server Error",
                description: "An unexpected database or server error occurred. Details are logged server-side but not exposed in the response.",
              },
            ]}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Error Response Examples</h3>
          <CodeBlock
            language="json"
            code={`// 400 — validation failure
{
  "error": "validation failed",
  "details": {
    "field": "published_at",
    "message": "invalid date value: 'not-a-date'"
  }
}

// 401 — missing or expired token
{
  "error": "unauthorized"
}

// 404 — record not found or not owned
{
  "error": "record not found"
}`}
          />
        </section>

        <PageNav href="/modules/data" />
      </div>
    </DocsLayout>
  );
}
