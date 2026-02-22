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
  { id: "audit-entry", title: "Audit Entry" },
  { id: "query-audit-log", title: "Query Audit Log" },
  { id: "use-cases", title: "Use Cases" },
];

export default function AuditLogsPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

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
            Immutable audit trail for all data mutations, implemented via PostgreSQL triggers ensuring 100% capture rate.
          </p>
        </div>

        {/* Outcomes */}
        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "Unbypassable tracking: Because the audit log is written by a database trigger within the same transaction, no data mutation can bypass it — even if performed directly in the database.",
              "Instant compliance: Satisfy SOC 2, HIPAA, and GDPR with a tamper-evident, chronological record of every INSERT, UPDATE, and DELETE across your entire platform.",
              "Time-travel debugging: Inspect the before/after snapshot of any record to understand exactly when and how a change occurred in production."
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
            RootCX automatically installs a PostgreSQL <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">AFTER INSERT OR UPDATE OR DELETE</code> trigger
            on every entity table provisioned from your manifest. Each trigger calls a shared PL/pgSQL function —{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system.audit_trigger_fn()</code> —
            that writes a structured log entry to the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system.audit_log</code> table,
            capturing the full old and new record as JSONB. Triggers follow the naming convention{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">audit_&lt;schema&gt;_&lt;table&gt;</code>{" "}
            (e.g. <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">audit_public_posts</code>).
          </p>
          <p className="text-muted-foreground leading-7">
            Because the trigger fires within the same transaction as the original operation, it is{" "}
            <strong className="text-foreground font-medium">impossible to bypass</strong> via
            the API. If the data changes, the audit entry is written. If the transaction rolls back,
            the audit entry rolls back with it — no phantom records.
          </p>

          <Callout variant="info" title="Append-Only">
            Audit log entries cannot be deleted, modified, or truncated via the RootCX API. The table
            has no DELETE or UPDATE endpoint. Entries can only be created by triggers and read
            through the query API.
          </Callout>
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

          <h3 className="text-lg font-semibold text-foreground mt-2">Example Entry</h3>
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
}`}
          />
        </section>

        {/* Query Audit Log */}
        <section className="flex flex-col gap-4" id="query-audit-log">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Query Audit Log</h2>
          <p className="text-muted-foreground leading-7">
            The audit log query endpoint returns entries in reverse chronological order (newest first).
            Results can be filtered by application and entity table. This endpoint requires
            administrator-level authentication.
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
                name: "limit",
                type: "integer",
                required: false,
                description: "Maximum number of entries to return. Defaults to 100. Minimum 1, maximum 1000.",
              },
              {
                name: "app_id",
                type: "string",
                required: false,
                description: "Filter entries by table_schema, scoping results to tables belonging to a specific application.",
              },
              {
                name: "entity",
                type: "string",
                required: false,
                description: "Filter by table_name. Must match the exact table name (e.g. posts, orders).",
              },
            ]}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Example</h3>
          <CodeBlock
            language="bash"
            code={`# Get the 50 most recent audit entries
curl "http://localhost:9100/api/v1/audit?limit=50" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response</h3>
          <CodeBlock
            language="json"
            code={`[
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
]`}
          />
        </section>

        {/* Use Cases */}
        <section className="flex flex-col gap-4" id="use-cases">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Use Cases</h2>

          <h3 className="text-lg font-semibold text-foreground mt-2">Compliance Auditing</h3>
          <p className="text-muted-foreground leading-7">
            Regulations like SOC 2, GDPR, and HIPAA require demonstrating who changed what data
            and when. The audit log provides a tamper-evident, chronological record of every data mutation
            that satisfies most compliance audit requirements. Query by entity and time range to produce
            reports for auditors.
          </p>

          <h3 className="text-lg font-semibold text-foreground mt-2">Debugging Data Changes</h3>
          <p className="text-muted-foreground leading-7">
            When a record is in an unexpected state, the audit log lets you reconstruct exactly how it got
            there. Filter by entity and record ID to retrieve the complete mutation history with full
            before/after snapshots for each change.
          </p>
          <CodeBlock
            language="bash"
            code={`# Retrieve recent audit entries for the orders table
curl "http://localhost:9100/api/v1/audit?entity=orders&limit=20" \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <Callout variant="warning" title="Long-Term Retention">
            The audit log grows indefinitely. For long-running production applications, implement a
            retention policy by partitioning the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system.audit_log</code>{" "}
            table by month and archiving or dropping old partitions according to your regulatory requirements.
            RootCX does not automatically purge audit entries.
          </Callout>
        </section>

        <PageNav href="/modules/audit" />

      </div>
    </DocsLayout>
  );
}
