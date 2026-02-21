import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "What is a Data Contract?" },
    { id: "entity-definition", title: "Entity Definition" },
    { id: "auto-managed-columns", title: "Auto-Managed Columns" },
    { id: "field-types", title: "All Field Types" },
    { id: "entity-link", title: "entity_link — Foreign Keys" },
    { id: "array-types", title: "Array Types" },
    { id: "enums", title: "Enums & CHECK Constraints" },
    { id: "default-values", title: "Default Values" },
    { id: "complete-example", title: "Complete Example" },
    { id: "schema-sync", title: "Schema Sync Behavior" },
    { id: "related", title: "Related Pages" },
];

export default function DataContractPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/concepts/manifest" className="hover:text-foreground transition-colors">Core Concepts</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Data Contract</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Data Contract</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        The data contract is the section of the App Manifest that defines your application's complete data model. Each entry describes an entity — its name, fields, types, constraints, and relationships — and the runtime automatically provisions and maintains the corresponding PostgreSQL schema.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">What is a Data Contract?</h2>
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">data contract</strong> is the authoritative description of what data your application owns and how it is structured. It is the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">dataContract</code> array inside the <Link href="/concepts/manifest" className="text-primary hover:underline">App Manifest</Link>.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Unlike traditional ORM schema files or raw SQL migrations, a RootCX data contract is entirely declarative. You describe <em>what</em> your data looks like — not <em>how</em> to get the database there. The <strong className="text-foreground font-medium">Schema Sync Engine</strong> computes the difference between your contract and the current live schema, then executes the minimum DDL required to reconcile them.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The data contract interacts closely with the permissions system: policy definitions reference entities by their <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">entityName</code>, and the runtime generates entity-aware API routes automatically from the contract. This means adding a new entity to your contract immediately makes it available via the REST Data API and secures it with your RBAC policies — no additional code required.
                    </p>

                    <CodeBlock language="json" filename="manifest.json — dataContract" code={`"dataContract": [
  {
    "entityName": "projects",
    "fields": [
      { "name": "title",       "type": "text",    "required": true },
      { "name": "status",      "type": "text",    "enumValues": ["active", "archived"], "defaultValue": "active" },
      { "name": "budget",      "type": "number" },
      { "name": "deadline",    "type": "date" },
      { "name": "is_public",   "type": "boolean", "defaultValue": "false" }
    ]
  }
]`} />
                </section>

                {/* Entity Definition */}
                <section className="flex flex-col gap-4" id="entity-definition">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Entity Definition</h2>
                    <p className="text-muted-foreground leading-7">
                        Each entry in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">dataContract</code> array is an <strong className="text-foreground font-medium">EntityContract</strong>. It maps directly to a PostgreSQL table in the application's dedicated schema (named after the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">appId</code>).
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "entityName",
                            type: "string",
                            required: true,
                            description: "The name of the entity. Used as the PostgreSQL table name, in REST API routes (/api/v1/data/{entityName}), and in policy definitions. Must be unique within the manifest. snake_case is recommended.",
                        },
                        {
                            name: "fields",
                            type: "FieldContract[]",
                            required: true,
                            description: "An ordered array of field definitions. Each field becomes a column in the PostgreSQL table, in addition to the four auto-managed system columns.",
                        },
                    ]} />

                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">entityName</code> must be stable. Renaming an entity in the manifest creates a new table and leaves the old one in place — it does not perform a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">RENAME TABLE</code>. If you need to rename a table, do so manually and update the manifest to match.
                    </p>

                    <Callout variant="tip" title="Naming convention">
                        Use lowercase snake_case for entity names (e.g. <code>work_orders</code>, <code>invoice_line_items</code>). This matches PostgreSQL conventions and ensures your manifest, database, and API routes are consistent.
                    </Callout>
                </section>

                {/* Auto-Managed Columns */}
                <section className="flex flex-col gap-4" id="auto-managed-columns">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Auto-Managed Columns</h2>
                    <p className="text-muted-foreground leading-7">
                        The runtime automatically adds four system columns to every entity table. You do not need to declare them in your data contract, and you must not declare fields with these names. These columns are present on every row and available in all API responses.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Column</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">PostgreSQL Type</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Behavior</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">id</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">UUID</td>
                                    <td className="px-4 py-3 text-muted-foreground">Primary key. Generated automatically using <code className="font-mono text-xs">gen_random_uuid()</code> on insert. Never writable by the client.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">created_at</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TIMESTAMPTZ</td>
                                    <td className="px-4 py-3 text-muted-foreground">Set to the current UTC timestamp on insert. Immutable after creation. Never writable by the client.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">updated_at</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TIMESTAMPTZ</td>
                                    <td className="px-4 py-3 text-muted-foreground">Automatically updated to the current UTC timestamp on every write. Maintained by a PostgreSQL trigger installed during schema sync.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">owner_id</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">UUID</td>
                                    <td className="px-4 py-3 text-muted-foreground">Set to the authenticated user's ID on insert. Used by <code className="font-mono text-xs">ownerOnly</code> policies to implement row-level access control.</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>

                    <Callout variant="note" title="owner_id and anonymous operations">
                        If a row is created through an unauthenticated request (where the runtime allows it), <code>owner_id</code> is set to <code>NULL</code>. Owner-only policies will not match these rows for any user, effectively making them unreadable via the user-scoped API.
                    </Callout>
                </section>

                {/* All Field Types */}
                <section className="flex flex-col gap-4" id="field-types">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">All Field Types</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX provides a set of semantic field types that map to PostgreSQL column types. Each type is designed to be self-describing and safe: the runtime rejects API payloads that violate type expectations before they reach the database.
                    </p>

                    <div className="flex flex-col gap-6">

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">text</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">TEXT</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                The default string type. Stores variable-length Unicode text with no length limit. Appropriate for names, slugs, descriptions, rich text, and all short free-form values. For structured string-based values (e.g. status codes), prefer pairing <code className="font-mono text-xs">text</code> with <code className="font-mono text-xs">enumValues</code>.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "description", "type": "text" }`} />
                        </div>

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">number</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">NUMERIC</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                Arbitrary-precision decimal. Uses PostgreSQL's <code className="font-mono text-xs">NUMERIC</code> type, which avoids floating-point rounding errors. Suitable for currency amounts, percentages, scores, quantities, and any value where exact decimal representation matters. JSON API values are transmitted as JSON numbers and stored without precision loss.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "price",    "type": "number", "required": true }
{ "name": "quantity", "type": "number", "defaultValue": "1" }`} />
                        </div>

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">boolean</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">BOOLEAN</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                A true/false flag. Stored as a native PostgreSQL boolean. JSON API accepts <code className="font-mono text-xs">true</code> or <code className="font-mono text-xs">false</code>. Use for feature flags, opt-in fields, binary toggles, and soft-deletion markers.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "is_active",   "type": "boolean", "defaultValue": "true" }
{ "name": "is_verified", "type": "boolean", "defaultValue": "false" }`} />
                        </div>

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">date</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">DATE</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                A calendar date with no time or timezone component. API values must be ISO 8601 date strings (<code className="font-mono text-xs">YYYY-MM-DD</code>). Use for birthdays, due dates, contract start/end dates, and any value where the time of day is not meaningful.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "due_date",       "type": "date" }
{ "name": "contract_start", "type": "date", "required": true }`} />
                        </div>

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">timestamp</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">TIMESTAMPTZ</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                A point in time with timezone awareness. Stored in UTC internally. API values must be ISO 8601 datetime strings (<code className="font-mono text-xs">YYYY-MM-DDTHH:MM:SSZ</code>). Use for event times, scheduled jobs, audit timestamps, and all precision-sensitive time values.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "scheduled_at",     "type": "timestamp" }
{ "name": "last_synced_at",  "type": "timestamp" }`} />
                        </div>

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">json</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">JSONB</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                Arbitrary JSON stored as binary JSONB. PostgreSQL's JSONB format supports GIN indexing on nested keys, containment queries, and key-existence checks — all available for use in worker scripts. Ideal for flexible metadata, configuration blobs, third-party webhook payloads, and schema-less extension points.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "metadata",  "type": "json" }
{ "name": "settings",  "type": "json" }
{ "name": "raw_event", "type": "json" }`} />
                        </div>

                        <div className="rounded-lg border border-border bg-white/[0.01] p-5">
                            <div className="flex items-center gap-2 mb-3">
                                <code className="rounded bg-primary/10 px-2 py-0.5 font-mono text-sm text-primary">file</code>
                                <span className="text-xs text-muted-foreground">→ PostgreSQL <code className="font-mono">TEXT</code></span>
                            </div>
                            <p className="text-sm text-muted-foreground leading-6">
                                A file reference field. Stored as TEXT containing the file path or URL, but the runtime treats uploads to this field specially: multipart form data is accepted by the Data API, the file is persisted to the configured storage backend, and the resulting URL or path is written to the column. Suitable for profile pictures, document attachments, and media assets. The upload size limit is 50 MB by default.
                            </p>
                            <CodeBlock language="json" code={`{ "name": "avatar",   "type": "file" }
{ "name": "document", "type": "file" }`} />
                        </div>

                    </div>
                </section>

                {/* entity_link */}
                <section className="flex flex-col gap-4" id="entity-link">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">entity_link — Foreign Keys</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">entity_link</code> type creates a foreign-key relationship between two entities. The column stores a UUID that references the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">id</code> of a row in another entity's table.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        When you declare an <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">entity_link</code> field, you must also provide a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">references</code> property naming the target entity. The runtime generates a PostgreSQL <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">FOREIGN KEY</code> constraint with <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ON DELETE SET NULL</code> semantics — deleting the parent row sets the link field to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">NULL</code> in all child rows rather than cascading the delete.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "type",
                            type: "\"entity_link\"",
                            required: true,
                            description: "Must be the string \"entity_link\".",
                        },
                        {
                            name: "references",
                            type: "string",
                            required: true,
                            description: "The entityName of the target entity within the same data contract. The referenced entity must exist in the manifest.",
                        },
                    ]} />

                    <CodeBlock language="json" filename="entity_link example" code={`{
  "entityName": "tasks",
  "fields": [
    { "name": "title",      "type": "text",        "required": true },
    { "name": "project_id", "type": "entity_link", "references": "projects" },
    { "name": "assignee_id","type": "entity_link", "references": "team_members" }
  ]
}`} />

                    <p className="text-muted-foreground leading-7">
                        This generates the following SQL:
                    </p>

                    <CodeBlock language="sql" code={`ALTER TABLE crm_app.tasks
  ADD COLUMN project_id  UUID REFERENCES crm_app.projects(id) ON DELETE SET NULL,
  ADD COLUMN assignee_id UUID REFERENCES crm_app.team_members(id) ON DELETE SET NULL;`} />

                    <Callout variant="info" title="Cross-entity queries">
                        Foreign keys are enforced at the database level. To query related data across entities in a worker script, use standard SQL joins with the entity names as table names in the app schema.
                    </Callout>
                </section>

                {/* Array Types */}
                <section className="flex flex-col gap-4" id="array-types">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Array Types</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX supports two array field types: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">[text]</code> and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">[number]</code>. These map to PostgreSQL native array types (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">TEXT[]</code> and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">NUMERIC[]</code> respectively) and are transmitted as JSON arrays in the API.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Type</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">PostgreSQL</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Use Cases</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Example API Value</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">[text]</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TEXT[]</td>
                                    <td className="px-4 py-3 text-muted-foreground">Tags, labels, skills, categories, multi-select options</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">["sales", "vip", "hot"]</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">[number]</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">NUMERIC[]</td>
                                    <td className="px-4 py-3 text-muted-foreground">Score vectors, coordinates, ordered quantities, rating arrays</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">[1.5, 2.0, 99.9]</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>

                    <CodeBlock language="json" filename="Array field examples" code={`{ "name": "tags",        "type": "[text]" }
{ "name": "categories",  "type": "[text]" }
{ "name": "scores",      "type": "[number]" }`} />

                    <Callout variant="tip" title="Querying arrays in workers">
                        PostgreSQL array operators work natively in worker script SQL. Use <code>@&gt;</code> (contains), <code>&amp;&amp;</code> (overlaps), and <code>ANY()</code> for efficient array filtering. For example: <code>WHERE tags @&gt; ARRAY['vip']</code>.
                    </Callout>
                </section>

                {/* Enums */}
                <section className="flex flex-col gap-4" id="enums">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Enums & CHECK Constraints</h2>
                    <p className="text-muted-foreground leading-7">
                        For <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">text</code> fields that should only accept a fixed set of values, add the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">enumValues</code> property. The runtime generates a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">CHECK</code> constraint that enforces the allowed values at the database level — no application-level validation needed.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Using <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">CHECK</code> constraints rather than PostgreSQL <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ENUM</code> types makes adding and removing values straightforward: the schema sync engine can drop the old constraint and add a new one without requiring a full type replacement.
                    </p>

                    <CodeBlock language="json" filename="Enum field definition" code={`{
  "name": "status",
  "type": "text",
  "enumValues": ["draft", "active", "paused", "archived"],
  "defaultValue": "draft"
}`} />

                    <p className="text-muted-foreground leading-7">Generated SQL:</p>

                    <CodeBlock language="sql" code={`ALTER TABLE crm_app.projects
  ADD COLUMN status TEXT NOT NULL DEFAULT 'draft'
    CHECK (status IN ('draft', 'active', 'paused', 'archived'));`} />

                    <p className="text-muted-foreground leading-7">
                        When you update the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">enumValues</code> array in the manifest and redeploy, the schema sync engine:
                    </p>

                    <ol className="flex flex-col gap-2 text-muted-foreground leading-7 list-decimal list-inside">
                        <li>Drops the existing CHECK constraint by name.</li>
                        <li>Adds a new CHECK constraint with the updated values.</li>
                        <li>Does not modify existing rows — existing data is not validated against the new constraint retroactively.</li>
                    </ol>

                    <Callout variant="warning" title="Removing enum values">
                        If you remove a value from <code>enumValues</code>, any existing rows that contain the removed value will violate the new CHECK constraint. Ensure you migrate existing data before removing enum values from your manifest.
                    </Callout>
                </section>

                {/* Default Values */}
                <section className="flex flex-col gap-4" id="default-values">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Default Values</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">defaultValue</code> property sets the SQL <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">DEFAULT</code> clause for a column. It is always a string in the manifest, and the runtime converts it to the appropriate SQL literal based on the field type.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Field Type</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">defaultValue (JSON string)</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Generated SQL DEFAULT</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">text</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"draft"</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DEFAULT 'draft'</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">number</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"0"</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DEFAULT 0</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">boolean</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"false"</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DEFAULT false</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">timestamp</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"now()"</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DEFAULT NOW()</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">json</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{'{"key": "value"}'}</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{'DEFAULT \'{"key":"value"}\'::jsonb'}</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>

                    <Callout variant="info" title="Defaults apply on INSERT">
                        Default values are applied by PostgreSQL on insert when the field is omitted from the API request body. They are not applied retroactively to existing rows when you add a <code>defaultValue</code> to an existing column in the manifest.
                    </Callout>
                </section>

                {/* Complete Example */}
                <section className="flex flex-col gap-4" id="complete-example">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Complete Example</h2>
                    <p className="text-muted-foreground leading-7">
                        The following data contract defines a project management application with four related entities: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">clients</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">projects</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">tasks</code>, and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">time_entries</code>. It demonstrates foreign keys, enums, array types, boolean fields, and default values.
                    </p>

                    <CodeBlock language="json" filename="manifest.json — full dataContract" code={`"dataContract": [
  {
    "entityName": "clients",
    "fields": [
      { "name": "name",         "type": "text",      "required": true },
      { "name": "email",        "type": "text",      "required": true },
      { "name": "phone",        "type": "text" },
      { "name": "website",      "type": "text" },
      { "name": "industry",     "type": "text",
        "enumValues": ["technology", "finance", "healthcare", "retail", "other"] },
      { "name": "logo",         "type": "file" },
      { "name": "tags",         "type": "[text]" },
      { "name": "is_active",    "type": "boolean",   "defaultValue": "true" },
      { "name": "metadata",     "type": "json" }
    ]
  },
  {
    "entityName": "projects",
    "fields": [
      { "name": "title",        "type": "text",      "required": true },
      { "name": "description",  "type": "text" },
      { "name": "status",       "type": "text",
        "enumValues": ["planning", "active", "on_hold", "completed", "cancelled"],
        "defaultValue": "planning" },
      { "name": "priority",     "type": "text",
        "enumValues": ["low", "medium", "high", "critical"],
        "defaultValue": "medium" },
      { "name": "budget",       "type": "number" },
      { "name": "spent",        "type": "number",    "defaultValue": "0" },
      { "name": "start_date",   "type": "date" },
      { "name": "deadline",     "type": "date" },
      { "name": "tags",         "type": "[text]" },
      { "name": "is_billable",  "type": "boolean",   "defaultValue": "true" },
      { "name": "client_id",    "type": "entity_link", "references": "clients" }
    ]
  },
  {
    "entityName": "tasks",
    "fields": [
      { "name": "title",        "type": "text",      "required": true },
      { "name": "description",  "type": "text" },
      { "name": "status",       "type": "text",
        "enumValues": ["todo", "in_progress", "in_review", "done"],
        "defaultValue": "todo" },
      { "name": "priority",     "type": "text",
        "enumValues": ["low", "medium", "high"],
        "defaultValue": "medium" },
      { "name": "estimated_hours", "type": "number" },
      { "name": "due_date",     "type": "date" },
      { "name": "is_blocked",   "type": "boolean",   "defaultValue": "false" },
      { "name": "labels",       "type": "[text]" },
      { "name": "project_id",   "type": "entity_link", "references": "projects" }
    ]
  },
  {
    "entityName": "time_entries",
    "fields": [
      { "name": "description",  "type": "text" },
      { "name": "hours",        "type": "number",    "required": true },
      { "name": "hourly_rate",  "type": "number" },
      { "name": "logged_at",    "type": "timestamp", "required": true },
      { "name": "is_invoiced",  "type": "boolean",   "defaultValue": "false" },
      { "name": "task_id",      "type": "entity_link", "references": "tasks" },
      { "name": "project_id",   "type": "entity_link", "references": "projects" }
    ]
  }
]`} />
                </section>

                {/* Schema Sync */}
                <section className="flex flex-col gap-4" id="schema-sync">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Schema Sync Behavior</h2>
                    <p className="text-muted-foreground leading-7">
                        Every manifest deployment triggers the <strong className="text-foreground font-medium">Schema Sync Engine</strong>. The engine performs a deterministic diff between the incoming manifest and the live PostgreSQL schema, then generates and executes the required DDL within a transaction. If any DDL statement fails, the transaction is rolled back and the deployment is rejected.
                    </p>

                    <div className="flex flex-col gap-3">
                        {[
                            {
                                scenario: "New entity added",
                                action: "CREATE TABLE with all declared fields plus the four system columns. Indexes and triggers are created automatically.",
                                safe: true,
                            },
                            {
                                scenario: "New field added to existing entity",
                                action: "ALTER TABLE … ADD COLUMN. If the field has required: true and no defaultValue, the column is added with a NULL constraint, which may fail if the table has existing rows.",
                                safe: true,
                            },
                            {
                                scenario: "required changed from false to true",
                                action: "ALTER TABLE … ALTER COLUMN … SET NOT NULL. Will fail if any existing rows have NULL in that column.",
                                safe: false,
                            },
                            {
                                scenario: "defaultValue added or changed",
                                action: "ALTER TABLE … ALTER COLUMN … SET DEFAULT. Applied to new inserts only.",
                                safe: true,
                            },
                            {
                                scenario: "enumValues updated",
                                action: "DROP CONSTRAINT + ADD CONSTRAINT. New rows are validated. Existing rows are not re-validated.",
                                safe: false,
                            },
                            {
                                scenario: "Field removed from manifest",
                                action: "No action. The column remains in the database. RootCX never drops columns automatically.",
                                safe: true,
                            },
                            {
                                scenario: "Entity removed from manifest",
                                action: "No action. The table remains in the database. RootCX never drops tables automatically.",
                                safe: true,
                            },
                            {
                                scenario: "entity_link references changed",
                                action: "DROP CONSTRAINT + ADD CONSTRAINT with the new reference target.",
                                safe: false,
                            },
                        ].map(({ scenario, action, safe }) => (
                            <div key={scenario} className="rounded-lg border border-border bg-white/[0.01] p-4 flex gap-4">
                                <div className={`mt-0.5 h-2 w-2 shrink-0 rounded-full ${safe ? "bg-green-500" : "bg-yellow-500"}`} />
                                <div className="flex flex-col gap-1">
                                    <p className="text-sm font-medium text-foreground">{scenario}</p>
                                    <p className="text-xs text-muted-foreground leading-relaxed">{action}</p>
                                </div>
                            </div>
                        ))}
                    </div>

                    <Callout variant="warning" title="Adding NOT NULL to a populated table">
                        If you set <code>required: true</code> on an existing field that already has NULL values in the database, the schema sync will fail. Backfill the column with a valid value before marking it as required, or add a <code>defaultValue</code> at the same time to allow PostgreSQL to fill in existing NULLs.
                    </Callout>
                </section>

                {/* Related */}
                <section className="flex flex-col gap-4" id="related">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Related Pages</h2>
                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        {[
                            { href: "/concepts/manifest", title: "App Manifest", desc: "The full manifest structure including top-level fields and the permissions block." },
                            { href: "/concepts/permissions", title: "Roles & Permissions", desc: "How entity names from the data contract are used in RBAC policy definitions." },
                            { href: "/modules/data", title: "Data Management", desc: "The REST API for reading and writing entity data." },
                            { href: "/concepts/runtime", title: "Engine & Runtime", desc: "How the runtime boots and applies the data contract to PostgreSQL." },
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

                <PageNav href="/concepts/data-contract" />
            </div>
        </DocsLayout>
    );
}
