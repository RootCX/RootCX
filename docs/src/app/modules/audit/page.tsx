import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const tocItems = [
  { id: "overview", title: "Overview" },
  { id: "how-it-works", title: "How It Works" },
  { id: "audit-entry", title: "Audit Entry" },
  { id: "query-audit-log", title: "Query Audit Log" },
  { id: "database-schema", title: "Database Schema" },
  { id: "trigger-function", title: "Trigger Function" },
  { id: "use-cases", title: "Use Cases" },
];

export default function AuditLogsPage() {
  return (
    <DocsLayout>
      <div className="flex gap-16 min-h-screen">
        <div className="flex-1 max-w-3xl py-10 px-2 flex flex-col gap-12">

          {/* Breadcrumb */}
          <div className="flex items-center gap-1.5 text-sm text-muted-foreground">
            <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
            <ChevronRight className="w-3.5 h-3.5" />
            <Link href="/modules" className="hover:text-foreground transition-colors">Native Modules</Link>
            <ChevronRight className="w-3.5 h-3.5" />
            <span className="text-foreground">Audit Logs</span>
          </div>

          {/* Title */}
          <div className="flex flex-col gap-3">
            <h1 className="text-4xl font-bold tracking-tight">Audit Logs</h1>
            <p className="text-lg text-muted-foreground leading-7">
              Immutable audit trail for all data mutations, implemented via PostgreSQL triggers. Every INSERT,
              UPDATE, and DELETE on every entity table is automatically captured with full before/after snapshots.
            </p>
          </div>

          {/* Overview */}
          <section className="flex flex-col gap-4" id="overview">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
            <p className="text-muted-foreground leading-7">
              RootCX automatically installs a PostgreSQL trigger on every entity table provisioned from your
              manifest. The trigger fires after every data-modifying operation and writes a structured log
              entry — including the full old and new record — to a central{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">audit_logs</code> table.
            </p>
            <p className="text-muted-foreground leading-7">
              Because the audit log is written by a database trigger within the same transaction as the
              original operation, it is <strong className="text-foreground font-medium">impossible to bypass</strong> via
              the API. If the data changes, the audit log changes too. If the transaction rolls back, the
              audit log entry rolls back with it — no phantom records.
            </p>
            <p className="text-muted-foreground leading-7">
              The audit log is designed for compliance, debugging, and undo workflows. It captures enough
              information to reconstruct the full history of any record, identify who changed what, and
              roll back unintended mutations in your application logic.
            </p>

            <Callout variant="info" title="Append-Only">
              Audit log entries cannot be deleted, modified, or truncated via the RootCX API. The table
              has no DELETE or UPDATE endpoint. Entries can only be created (by triggers) and read (by the
              query API). Direct database access with superuser privileges is the only way to remove entries,
              which should be reserved for legal retention obligations.
            </Callout>
          </section>

          {/* How It Works */}
          <section className="flex flex-col gap-4" id="how-it-works">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">How It Works</h2>
            <p className="text-muted-foreground leading-7">
              When RootCX provisions an entity table at startup (or when the schema sync engine runs), it
              also installs a trigger on that table. The trigger calls a shared PL/pgSQL function —{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_audit_trigger()</code> —
              that is responsible for writing the log entry.
            </p>

            <h3 className="text-lg font-semibold text-foreground mt-2">Trigger Lifecycle</h3>
            <ol className="flex flex-col gap-2 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">1.</span>
                <span>A client sends a POST, PATCH, or DELETE request to the data API.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">2.</span>
                <span>The runtime executes the corresponding INSERT, UPDATE, or DELETE SQL within a transaction.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">3.</span>
                <span>PostgreSQL fires the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">AFTER INSERT OR UPDATE OR DELETE</code> trigger on the entity table.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">4.</span>
                <span>The trigger function serializes <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">OLD</code> and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">NEW</code> row values as JSONB and inserts a row into <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">audit_logs</code>.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">5.</span>
                <span>The transaction commits. Both the data change and the audit log entry become durable simultaneously.</span>
              </li>
            </ol>

            <h3 className="text-lg font-semibold text-foreground mt-2">What Fires the Trigger</h3>
            <PropertiesTable
              properties={[
                {
                  name: "INSERT",
                  type: "POST /collections/{entity}",
                  description: "Fired after a new record is created. OLD is NULL. NEW contains the full inserted row.",
                },
                {
                  name: "UPDATE",
                  type: "PATCH /collections/{entity}/{id}",
                  description: "Fired after a record is patched. OLD contains the row before the update. NEW contains the row after.",
                },
                {
                  name: "DELETE",
                  type: "DELETE /collections/{entity}/{id}",
                  description: "Fired after a record is deleted. OLD contains the full row that was deleted. NEW is NULL.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Performance</h3>
            <p className="text-muted-foreground leading-7">
              The trigger executes within the same transaction as the data operation — it is not truly
              asynchronous — but the overhead is minimal for typical record sizes. The{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">audit_logs</code> table
              is append-only and indexed on <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">changed_at</code>,{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">table_name</code>, and{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">record_id</code> to
              support efficient querying. For write-heavy workloads, consider partitioning the audit table
              by <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">changed_at</code> range
              for long-term retention.
            </p>
          </section>

          {/* Audit Entry */}
          <section className="flex flex-col gap-4" id="audit-entry">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Audit Entry</h2>
            <p className="text-muted-foreground leading-7">
              Each audit log entry captures a complete snapshot of what changed, when it changed, and on
              which table and record.
            </p>

            <PropertiesTable
              properties={[
                {
                  name: "id",
                  type: "BIGINT",
                  description: "Auto-incrementing identity column. Globally monotonically increasing — a higher ID always means a later event.",
                },
                {
                  name: "table_schema",
                  type: "TEXT",
                  description: "The PostgreSQL schema containing the table. Typically 'public' for RootCX entity tables.",
                },
                {
                  name: "table_name",
                  type: "TEXT",
                  description: "The name of the entity table that was mutated. Corresponds to the entity name in your manifest.",
                },
                {
                  name: "record_id",
                  type: "TEXT",
                  description: "The ID of the affected record. Extracted from the NEW row on INSERT/UPDATE, and from the OLD row on DELETE.",
                },
                {
                  name: "operation",
                  type: "TEXT",
                  description: "One of INSERT, UPDATE, or DELETE. Exactly as reported by the TG_OP trigger variable.",
                },
                {
                  name: "old_record",
                  type: "JSONB",
                  description: "The full row before the change, serialized as JSONB. NULL for INSERT operations.",
                },
                {
                  name: "new_record",
                  type: "JSONB",
                  description: "The full row after the change, serialized as JSONB. NULL for DELETE operations.",
                },
                {
                  name: "changed_at",
                  type: "TIMESTAMPTZ",
                  description: "The timestamp at which the change occurred, set to NOW() within the trigger. Uses the database server clock in UTC.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Example Audit Entries</h3>
            <CodeBlock
              language="json"
              code={`// INSERT — new post created
{
  "id": 1001,
  "table_schema": "public",
  "table_name": "posts",
  "record_id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
  "operation": "INSERT",
  "old_record": null,
  "new_record": {
    "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
    "owner_id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
    "title": "Hello World",
    "body": "My first post.",
    "published": false,
    "created_at": "2025-01-15T10:30:00.000Z",
    "updated_at": "2025-01-15T10:30:00.000Z"
  },
  "changed_at": "2025-01-15T10:30:00.123Z"
}

// UPDATE — post published
{
  "id": 1002,
  "table_schema": "public",
  "table_name": "posts",
  "record_id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
  "operation": "UPDATE",
  "old_record": {
    "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
    "title": "Hello World",
    "published": false,
    "updated_at": "2025-01-15T10:30:00.000Z"
  },
  "new_record": {
    "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
    "title": "Hello World",
    "published": true,
    "updated_at": "2025-01-15T12:05:32.871Z"
  },
  "changed_at": "2025-01-15T12:05:32.910Z"
}

// DELETE — post removed
{
  "id": 1003,
  "table_schema": "public",
  "table_name": "posts",
  "record_id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
  "operation": "DELETE",
  "old_record": {
    "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
    "title": "Hello World",
    "published": true
  },
  "new_record": null,
  "changed_at": "2025-01-16T09:00:00.000Z"
}`}
            />
          </section>

          {/* Query Audit Log */}
          <section className="flex flex-col gap-4" id="query-audit-log">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Query Audit Log</h2>
            <p className="text-muted-foreground leading-7">
              The audit log query endpoint returns entries in reverse chronological order (newest first).
              Results can be filtered by application, entity table, and operation type. This endpoint
              requires administrator-level authentication.
            </p>

            <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
              <div className="flex items-center gap-3">
                <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
                <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                  /api/v1/audit
                </code>
              </div>
            </div>

            <h3 className="text-lg font-semibold text-foreground mt-2">Query Parameters</h3>
            <PropertiesTable
              properties={[
                {
                  name: "app_id",
                  type: "string",
                  required: false,
                  description: "Filter entries to tables belonging to a specific application. Maps to a table_name prefix convention.",
                },
                {
                  name: "entity",
                  type: "string",
                  required: false,
                  description: "Filter by table name (e.g. posts, orders). Must match the exact table_name column value.",
                },
                {
                  name: "operation",
                  type: "string",
                  required: false,
                  description: "Filter by operation type. Accepted values: INSERT, UPDATE, DELETE. Case-sensitive.",
                },
                {
                  name: "record_id",
                  type: "string",
                  required: false,
                  description: "Filter to audit entries for a specific record ID. Useful for retrieving the full history of a single record.",
                },
                {
                  name: "limit",
                  type: "integer",
                  required: false,
                  description: "Maximum number of entries to return. Defaults to 100. Maximum 1000.",
                },
                {
                  name: "before",
                  type: "integer",
                  required: false,
                  description: "Return entries with id less than this value. Use for cursor-based pagination by passing the smallest id from the previous response.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Examples</h3>
            <CodeBlock
              language="bash"
              code={`# Get the 50 most recent audit entries across all tables
curl "https://your-runtime.example.com/api/v1/audit?limit=50" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."

# Filter to DELETE operations on the posts table
curl "https://your-runtime.example.com/api/v1/audit?entity=posts&operation=DELETE" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."

# Get the full history of a specific record
curl "https://your-runtime.example.com/api/v1/audit?entity=posts&record_id=01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."

# Paginate — get next page using smallest id from previous response
curl "https://your-runtime.example.com/api/v1/audit?limit=100&before=1001" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
            <CodeBlock
              language="json"
              code={`{
  "data": [
    {
      "id": 1003,
      "table_schema": "public",
      "table_name": "posts",
      "record_id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
      "operation": "DELETE",
      "old_record": {
        "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
        "title": "Hello World",
        "published": true,
        "owner_id": "01926b3e-1234-7000-aaaa-bbbbccccdddd"
      },
      "new_record": null,
      "changed_at": "2025-01-16T09:00:00.000Z"
    },
    {
      "id": 1002,
      "table_schema": "public",
      "table_name": "posts",
      "record_id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
      "operation": "UPDATE",
      "old_record": {
        "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
        "title": "Hello World",
        "published": false
      },
      "new_record": {
        "id": "01926b3e-7c2d-7000-b1e2-3f4a5d6e7f8a",
        "title": "Hello World",
        "published": true
      },
      "changed_at": "2025-01-15T12:05:32.910Z"
    }
  ],
  "count": 2
}`}
            />
          </section>

          {/* Database Schema */}
          <section className="flex flex-col gap-4" id="database-schema">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Database Schema</h2>
            <p className="text-muted-foreground leading-7">
              The audit log table is provisioned automatically on runtime startup. It uses a BIGINT GENERATED
              ALWAYS AS IDENTITY primary key for guaranteed monotonic ordering without gaps from rollbacks.
            </p>
            <CodeBlock
              language="sql"
              code={`CREATE TABLE IF NOT EXISTS audit_logs (
  id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  table_schema TEXT         NOT NULL,
  table_name   TEXT         NOT NULL,
  record_id    TEXT         NOT NULL,
  operation    TEXT         NOT NULL CHECK (operation IN ('INSERT', 'UPDATE', 'DELETE')),
  old_record   JSONB,
  new_record   JSONB,
  changed_at   TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- Index for querying by table and time
CREATE INDEX IF NOT EXISTS audit_logs_table_changed_idx
  ON audit_logs (table_name, changed_at DESC);

-- Index for querying the full history of a specific record
CREATE INDEX IF NOT EXISTS audit_logs_record_idx
  ON audit_logs (table_name, record_id, changed_at DESC);

-- Index for time-range queries
CREATE INDEX IF NOT EXISTS audit_logs_changed_at_idx
  ON audit_logs (changed_at DESC);`}
            />
          </section>

          {/* Trigger Function */}
          <section className="flex flex-col gap-4" id="trigger-function">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Trigger Function</h2>
            <p className="text-muted-foreground leading-7">
              A single shared PL/pgSQL function handles all entity tables. The trigger on each table calls
              this function, which uses the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">TG_TABLE_SCHEMA</code>,{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">TG_TABLE_NAME</code>, and{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">TG_OP</code> special
              variables to record where and what the change was.
            </p>
            <CodeBlock
              language="sql"
              code={`-- Shared trigger function (created once at runtime startup)
CREATE OR REPLACE FUNCTION rootcx_audit_trigger()
RETURNS TRIGGER AS $$
DECLARE
  v_record_id TEXT;
  v_old_record JSONB;
  v_new_record JSONB;
BEGIN
  -- Extract record_id from whichever row is available
  IF TG_OP = 'DELETE' THEN
    v_record_id  := OLD.id::TEXT;
    v_old_record := to_jsonb(OLD);
    v_new_record := NULL;
  ELSIF TG_OP = 'INSERT' THEN
    v_record_id  := NEW.id::TEXT;
    v_old_record := NULL;
    v_new_record := to_jsonb(NEW);
  ELSE -- UPDATE
    v_record_id  := NEW.id::TEXT;
    v_old_record := to_jsonb(OLD);
    v_new_record := to_jsonb(NEW);
  END IF;

  INSERT INTO audit_logs (
    table_schema,
    table_name,
    record_id,
    operation,
    old_record,
    new_record,
    changed_at
  ) VALUES (
    TG_TABLE_SCHEMA,
    TG_TABLE_NAME,
    v_record_id,
    TG_OP,
    v_old_record,
    v_new_record,
    NOW()
  );

  RETURN NULL; -- AFTER trigger; return value is ignored
END;
$$ LANGUAGE plpgsql;

-- Per-entity trigger installed when each table is provisioned
-- (example for the 'posts' entity)
CREATE OR REPLACE TRIGGER posts_audit_trigger
  AFTER INSERT OR UPDATE OR DELETE ON posts
  FOR EACH ROW EXECUTE FUNCTION rootcx_audit_trigger();`}
            />

            <Callout variant="info" title="Trigger Installation">
              The trigger is automatically created for every entity defined in your manifest. If you add a
              new entity, the trigger is installed when the runtime starts or when the schema sync engine
              runs. You do not need to write or manage triggers manually.
            </Callout>
          </section>

          {/* Use Cases */}
          <section className="flex flex-col gap-4" id="use-cases">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Use Cases</h2>

            <h3 className="text-lg font-semibold text-foreground mt-2">Compliance Auditing</h3>
            <p className="text-muted-foreground leading-7">
              Regulations like SOC 2, GDPR, and HIPAA often require that you demonstrate who changed what data
              and when. The audit log provides a tamper-evident, chronological record of every data mutation
              that satisfies most compliance audit requirements. Query by entity and date range to produce
              reports for auditors.
            </p>
            <CodeBlock
              language="bash"
              code={`# Get all changes to the 'orders' table in the last 24 hours (via a Worker/RPC query)
SELECT *
FROM audit_logs
WHERE table_name = 'orders'
  AND changed_at >= NOW() - INTERVAL '24 hours'
ORDER BY changed_at DESC;`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Debugging Data Changes</h3>
            <p className="text-muted-foreground leading-7">
              When a record is in an unexpected state, the audit log lets you reconstruct exactly how it got
              there. Query by <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">record_id</code> to
              retrieve the complete mutation history of any record, with full before/after snapshots for
              each change.
            </p>
            <CodeBlock
              language="bash"
              code={`# Retrieve full history of a specific record
curl "https://your-runtime.example.com/api/v1/audit?entity=orders&record_id=ORDER-12345" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Undo Capability</h3>
            <p className="text-muted-foreground leading-7">
              Since every UPDATE audit entry stores the full{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">old_record</code>,
              you can implement an undo feature in your application by reading the most recent UPDATE entry
              for a record and re-applying the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">old_record</code> values
              via a PATCH request. Similarly, deleted records can be recovered by reading the DELETE entry's{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">old_record</code> and
              re-creating the record via POST.
            </p>
            <CodeBlock
              language="javascript"
              code={`// Worker: undo the last update to a record
export default async function undoLastUpdate(req) {
  const { entity, recordId } = req.body;

  // Find the most recent UPDATE entry for this record
  const { rows } = await db.query(
    \`SELECT old_record FROM audit_logs
     WHERE table_name = $1
       AND record_id  = $2
       AND operation  = 'UPDATE'
     ORDER BY changed_at DESC
     LIMIT 1\`,
    [entity, recordId]
  );

  if (!rows.length) {
    return { error: "no update history found" };
  }

  const { old_record } = rows[0];

  // Re-apply the old values
  const fields = Object.keys(old_record)
    .filter(k => !["id", "owner_id", "created_at"].includes(k));

  const setClause = fields
    .map((k, i) => \`\${k} = $\${i + 2}\`)
    .join(", ");

  await db.query(
    \`UPDATE \${entity} SET \${setClause}, updated_at = NOW() WHERE id = $1\`,
    [recordId, ...fields.map(k => old_record[k])]
  );

  return { success: true, restored: old_record };
}`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Change Notifications</h3>
            <p className="text-muted-foreground leading-7">
              Poll the audit log from a scheduled Worker to detect recent changes and trigger downstream
              workflows — send emails, push notifications, or webhook calls when specific entities are
              modified. Combined with the{" "}
              <Link href="/modules/jobs" className="text-foreground underline underline-offset-4 hover:text-muted-foreground transition-colors">Jobs module</Link>,
              you can build event-driven pipelines entirely within RootCX.
            </p>

            <Callout variant="warning" title="Long-Term Retention">
              The audit log grows indefinitely. For long-running production applications, implement a
              retention policy by partitioning the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">audit_logs</code> table
              by month and archiving or dropping old partitions according to your regulatory requirements.
              RootCX does not automatically purge audit entries.
            </Callout>
          </section>

        </div>

        <PageNav href="/modules/audit" />
      </div>
    </DocsLayout>
  );
}
