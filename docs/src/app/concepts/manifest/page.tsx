import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "What is a Manifest?" },
    { id: "top-level-fields", title: "Top-Level Fields" },
    { id: "permissions-block", title: "The Permissions Block" },
    { id: "roles", title: "Roles" },
    { id: "policies", title: "Policies" },
    { id: "data-contract-block", title: "The Data Contract Block" },
    { id: "field-types", title: "Field Types" },
    { id: "field-contract-properties", title: "FieldContract Properties" },
    { id: "complete-example", title: "Complete Example" },
    { id: "schema-migration", title: "Schema Auto-Migration" },
    { id: "related", title: "Related Pages" },
];

export default function ManifestPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/concepts/manifest" className="hover:text-foreground transition-colors">Core Concepts</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">App Manifest</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">App Manifest</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        The manifest is the single source of truth for your RootCX application. It is a JSON document that declares your app's identity, data model, and access-control rules — everything the runtime needs to provision and operate your application.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">What is a Manifest?</h2>
                    <p className="text-muted-foreground leading-7">
                        When you deploy an application to RootCX, the runtime reads a single JSON file — the <strong className="text-foreground font-medium">App Manifest</strong> — and uses it to create or update everything your application needs. This includes the PostgreSQL tables that back your data model, the RBAC roles and policies that control who can access what, and the metadata stored in the system schema that ties everything together.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The manifest follows a <strong className="text-foreground font-medium">declarative</strong> model: you describe the desired state, and the runtime reconciles the live database to match it. You never write SQL migrations by hand. Add a field to your manifest and redeploy — the runtime computes the diff and executes the necessary <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ALTER TABLE</code> statements automatically.
                    </p>

                    <Callout variant="tip" title="Manifest as code">
                        Store your manifest in version control alongside your worker scripts and Studio code. Every deployment is reproducible and auditable from a single JSON file.
                    </Callout>

                    <p className="text-muted-foreground leading-7">
                        At the top level, a manifest has four required fields and two structured blocks:
                    </p>

                    <CodeBlock language="json" filename="manifest.json" code={`{
  "appId": "crm-app",
  "name": "CRM Application",
  "version": "1.0.0",
  "description": "Customer relationship management for sales teams",
  "permissions": { ... },
  "dataContract": [ ... ]
}`} />
                </section>

                {/* Top-level fields */}
                <section className="flex flex-col gap-4" id="top-level-fields">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Top-Level Fields</h2>
                    <p className="text-muted-foreground leading-7">
                        The following fields appear at the root of every manifest document.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "appId",
                            type: "string",
                            required: true,
                            description: "A globally unique identifier for your application. Used as the PostgreSQL schema name and as the namespace for all system tables. Must be lowercase alphanumeric with hyphens (kebab-case). Cannot be changed after the first deploy without a full schema rename.",
                        },
                        {
                            name: "name",
                            type: "string",
                            required: true,
                            description: "A human-readable display name for the application shown in the Studio IDE and audit log entries.",
                        },
                        {
                            name: "version",
                            type: "string",
                            required: true,
                            description: "Semantic version string (e.g. \"1.0.0\"). The runtime stores this in the system schema and uses it for deployment history. Incrementing the version on each deploy is strongly recommended.",
                        },
                        {
                            name: "description",
                            type: "string",
                            required: false,
                            default: "\"\"",
                            description: "A short human-readable description of the application's purpose. Shown in the Studio overview panel.",
                        },
                        {
                            name: "permissions",
                            type: "PermissionsBlock",
                            required: true,
                            description: "Defines the RBAC model: roles, the default role, and the access policies that govern each role's capabilities. See the Permissions Block section below.",
                        },
                        {
                            name: "dataContract",
                            type: "EntityContract[]",
                            required: true,
                            description: "An array of entity definitions. Each entry declares a logical entity (table) with its fields, types, and constraints. See the Data Contract Block section below.",
                        },
                    ]} />

                    <Callout variant="warning" title="appId is immutable">
                        The <code>appId</code> becomes the PostgreSQL schema name. Changing it after the initial deployment requires migrating all data to a new schema, which is not handled automatically. Treat <code>appId</code> as permanent.
                    </Callout>
                </section>

                {/* Permissions Block */}
                <section className="flex flex-col gap-4" id="permissions-block">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">The Permissions Block</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permissions</code> object is the root of the RBAC configuration. It contains three properties:
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "roles",
                            type: "Role[]",
                            required: true,
                            description: "An array of role definitions. Each role is a named group that can inherit from other roles and be associated with policies.",
                        },
                        {
                            name: "defaultRole",
                            type: "string",
                            required: true,
                            description: "The name of the role automatically assigned to every new user upon registration. Must match one of the role names defined in the roles array.",
                        },
                        {
                            name: "policies",
                            type: "Policy[]",
                            required: true,
                            description: "An array of policy definitions. Each policy grants a role a set of actions on a specific entity (or all entities via the wildcard).",
                        },
                    ]} />

                    <CodeBlock language="json" filename="manifest.json — permissions" code={`"permissions": {
  "defaultRole": "member",
  "roles": [
    { "name": "admin" },
    { "name": "manager", "inherits": ["member"] },
    { "name": "member" }
  ],
  "policies": [
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
    }
  ]
}`} />
                </section>

                {/* Roles */}
                <section className="flex flex-col gap-4" id="roles">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Roles</h2>
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">role</strong> is a named group that can be assigned to users. Roles support single and multiple inheritance: a role can declare an <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">inherits</code> array listing parent role names. During policy evaluation, the runtime walks the inheritance graph and collects the union of all actions granted to the role and all its ancestors.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "name",
                            type: "string",
                            required: true,
                            description: "Unique identifier for the role within this application. Used in policy definitions and role assignments.",
                        },
                        {
                            name: "inherits",
                            type: "string[]",
                            required: false,
                            default: "[]",
                            description: "An array of role names that this role inherits from. The role gains all permissions of its parent roles in addition to its own.",
                        },
                    ]} />

                    <Callout variant="info" title="Role hierarchy is additive">
                        Inheritance only adds permissions — it never removes them. If a parent role can <code>delete</code> an entity, the child role also gains that capability. Design your role hierarchy from least-privileged to most-privileged.
                    </Callout>
                </section>

                {/* Policies */}
                <section className="flex flex-col gap-4" id="policies">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Policies</h2>
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">policy</strong> is the core unit of access control. It grants a specific role the ability to perform specific actions on a specific entity. The runtime evaluates policies at request time and enforces them before any SQL is executed.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "role",
                            type: "string",
                            required: true,
                            description: "The role this policy applies to. Must match a role name defined in the roles array.",
                        },
                        {
                            name: "entity",
                            type: "string",
                            required: true,
                            description: "The entity (table) this policy covers. Use the entity's entityName value. Use \"*\" to apply the policy to all entities in the data contract.",
                        },
                        {
                            name: "actions",
                            type: "string[]",
                            required: true,
                            description: "The list of permitted CRUD actions. Valid values: \"create\", \"read\", \"update\", \"delete\".",
                        },
                        {
                            name: "ownerOnly",
                            type: "boolean",
                            required: false,
                            default: "false",
                            description: "When true, read/update/delete operations are filtered to rows where owner_id equals the authenticated user's ID. Enables row-level ownership enforcement.",
                        },
                    ]} />

                    <Callout variant="note" title="Wildcard entity">
                        Setting <code>entity</code> to <code>"*"</code> creates a blanket policy across all entities in the data contract. This is useful for admin roles that need unrestricted access without enumerating every entity explicitly.
                    </Callout>
                </section>

                {/* Data Contract Block */}
                <section className="flex flex-col gap-4" id="data-contract-block">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">The Data Contract Block</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">dataContract</code> array is where you declare your application's data model. Each entry is an <strong className="text-foreground font-medium">EntityContract</strong> — a description of a logical entity (database table) and all of its fields.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The runtime translates each entity into a PostgreSQL table in the application's schema. All tables receive four auto-managed columns in addition to the fields you declare: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">id</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">created_at</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">updated_at</code>, and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id</code>.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "entityName",
                            type: "string",
                            required: true,
                            description: "The logical name for the entity. Used as the table name in PostgreSQL (snake_case recommended). Also used in policy definitions and API routes.",
                        },
                        {
                            name: "fields",
                            type: "FieldContract[]",
                            required: true,
                            description: "An ordered array of field definitions. Each field becomes a column in the PostgreSQL table.",
                        },
                    ]} />

                    <CodeBlock language="json" filename="manifest.json — dataContract entry" code={`{
  "entityName": "contacts",
  "fields": [
    { "name": "first_name", "type": "text", "required": true },
    { "name": "last_name",  "type": "text", "required": true },
    { "name": "email",      "type": "text", "required": true },
    { "name": "phone",      "type": "text" },
    { "name": "status",     "type": "text", "enumValues": ["lead", "prospect", "customer", "churned"], "defaultValue": "lead" },
    { "name": "score",      "type": "number", "defaultValue": "0" },
    { "name": "account_id", "type": "entity_link", "references": "accounts" }
  ]
}`} />
                </section>

                {/* Field Types */}
                <section className="flex flex-col gap-4" id="field-types">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Field Types</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX supports a set of high-level field types that map to specific PostgreSQL column types. Using these type aliases keeps your manifest readable and independent of database-specific syntax while ensuring correct storage semantics.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border my-2">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Type</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">PostgreSQL Mapping</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Description</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Example Value</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">text</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TEXT</td>
                                    <td className="px-4 py-3 text-muted-foreground">Variable-length string with no length limit. The most common type for names, emails, descriptions, and status codes.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"hello world"</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">number</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">NUMERIC</td>
                                    <td className="px-4 py-3 text-muted-foreground">Arbitrary-precision decimal number. Suitable for currency, scores, quantities, and any value where floating-point rounding is unacceptable.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">42, 3.14, 99.99</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">boolean</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">BOOLEAN</td>
                                    <td className="px-4 py-3 text-muted-foreground">True/false flag. Stored as a native PostgreSQL boolean. Useful for feature flags, opt-ins, and binary state.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">true, false</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">date</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">DATE</td>
                                    <td className="px-4 py-3 text-muted-foreground">Calendar date without time or timezone. Use for birthdays, due dates, and any value where the time of day is irrelevant.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"2024-12-31"</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">timestamp</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TIMESTAMPTZ</td>
                                    <td className="px-4 py-3 text-muted-foreground">Date and time with timezone. Stored in UTC internally. Use for event times, deadlines, and all audit-relevant moments.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"2024-12-31T23:59:59Z"</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">json</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">JSONB</td>
                                    <td className="px-4 py-3 text-muted-foreground">Arbitrary JSON stored as binary JSONB. Supports indexing on nested keys. Use for flexible metadata, settings blobs, or semi-structured data.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{"{ \"key\": \"val\" }"}</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">file</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TEXT</td>
                                    <td className="px-4 py-3 text-muted-foreground">Stores the path or URL of an uploaded file. The runtime handles multipart uploads and writes the resolved file reference to this column.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"/uploads/avatar.png"</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">entity_link</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">UUID REFERENCES</td>
                                    <td className="px-4 py-3 text-muted-foreground">A foreign key to another entity's primary key (UUID). Requires a <code className="font-mono text-xs">references</code> property naming the target entity. Enforces referential integrity at the database level.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">"uuid-v4-string"</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">[text]</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">TEXT[]</td>
                                    <td className="px-4 py-3 text-muted-foreground">An array of text strings. Stored as a native PostgreSQL array. Useful for tags, labels, multi-select values, and skill sets.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">["a", "b", "c"]</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">[number]</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">NUMERIC[]</td>
                                    <td className="px-4 py-3 text-muted-foreground">An array of numeric values. Useful for storing sets of scores, coordinates, or any ordered list of numbers.</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">[1, 2.5, 100]</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* FieldContract Properties */}
                <section className="flex flex-col gap-4" id="field-contract-properties">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">FieldContract Properties</h2>
                    <p className="text-muted-foreground leading-7">
                        Each entry in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">fields</code> array is a <strong className="text-foreground font-medium">FieldContract</strong> object. Below is the complete property reference.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "name",
                            type: "string",
                            required: true,
                            description: "The column name in PostgreSQL. Must be unique within the entity. Use snake_case for consistency with PostgreSQL conventions.",
                        },
                        {
                            name: "type",
                            type: "FieldType",
                            required: true,
                            description: "The field type. One of: text, number, boolean, date, timestamp, json, file, entity_link, [text], [number]. See the Field Types table above.",
                        },
                        {
                            name: "required",
                            type: "boolean",
                            required: false,
                            default: "false",
                            description: "When true, generates a NOT NULL constraint on the column. API writes that omit this field will be rejected with a 400 error.",
                        },
                        {
                            name: "defaultValue",
                            type: "string",
                            required: false,
                            description: "A default value expressed as a string. The runtime converts it to the appropriate SQL DEFAULT clause. For booleans use \"true\"/\"false\", for numbers use the numeric string, for text use the literal value.",
                        },
                        {
                            name: "enumValues",
                            type: "string[]",
                            required: false,
                            description: "An array of permitted string values. Generates a CHECK constraint ensuring only listed values can be written to this column. Only valid for text fields.",
                        },
                        {
                            name: "references",
                            type: "string",
                            required: false,
                            description: "Required when type is entity_link. The entityName of the target entity. Generates a REFERENCES constraint with ON DELETE SET NULL semantics.",
                        },
                        {
                            name: "isPrimaryKey",
                            type: "boolean",
                            required: false,
                            default: "false",
                            description: "Marks this field as the primary key. RootCX automatically adds a UUID primary key column (id) to every entity, so this property is rarely needed. Use only when defining a custom primary key.",
                        },
                    ]} />

                    <Callout variant="info" title="Auto-managed columns">
                        Every entity automatically receives four system columns that you do not need to declare: <code>id</code> (UUID, primary key, generated by default), <code>created_at</code> (TIMESTAMPTZ, set on insert), <code>updated_at</code> (TIMESTAMPTZ, updated on every write), and <code>owner_id</code> (UUID, set to the authenticated user's ID on creation). Do not declare fields with these names.
                    </Callout>
                </section>

                {/* Complete Example */}
                <section className="flex flex-col gap-4" id="complete-example">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Complete Example</h2>
                    <p className="text-muted-foreground leading-7">
                        The following manifest defines a CRM application with two entities — <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">contacts</code> and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">deals</code> — and a three-tier RBAC model with admin, manager, and member roles.
                    </p>

                    <CodeBlock language="json" filename="manifest.json" code={`{
  "appId": "crm-app",
  "name": "CRM Application",
  "version": "1.0.0",
  "description": "Customer relationship management for sales teams",

  "permissions": {
    "defaultRole": "member",
    "roles": [
      { "name": "admin" },
      { "name": "manager", "inherits": ["member"] },
      { "name": "member" }
    ],
    "policies": [
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
        "role": "manager",
        "entity": "deals",
        "actions": ["create", "read", "update", "delete"]
      },
      {
        "role": "member",
        "entity": "contacts",
        "actions": ["create", "read", "update"],
        "ownerOnly": true
      },
      {
        "role": "member",
        "entity": "deals",
        "actions": ["create", "read", "update"],
        "ownerOnly": true
      }
    ]
  },

  "dataContract": [
    {
      "entityName": "contacts",
      "fields": [
        { "name": "first_name",  "type": "text",    "required": true },
        { "name": "last_name",   "type": "text",    "required": true },
        { "name": "email",       "type": "text",    "required": true },
        { "name": "phone",       "type": "text" },
        { "name": "company",     "type": "text" },
        { "name": "status",      "type": "text",
          "enumValues": ["lead", "prospect", "customer", "churned"],
          "defaultValue": "lead" },
        { "name": "score",       "type": "number",  "defaultValue": "0" },
        { "name": "tags",        "type": "[text]" },
        { "name": "notes",       "type": "text" },
        { "name": "avatar",      "type": "file" },
        { "name": "metadata",    "type": "json" },
        { "name": "last_contacted_at", "type": "timestamp" }
      ]
    },
    {
      "entityName": "deals",
      "fields": [
        { "name": "title",       "type": "text",    "required": true },
        { "name": "value",       "type": "number",  "required": true },
        { "name": "currency",    "type": "text",
          "enumValues": ["USD", "EUR", "GBP", "JPY"],
          "defaultValue": "USD" },
        { "name": "stage",       "type": "text",
          "enumValues": ["prospecting", "qualification", "proposal", "negotiation", "closed_won", "closed_lost"],
          "defaultValue": "prospecting" },
        { "name": "probability", "type": "number",  "defaultValue": "0" },
        { "name": "close_date",  "type": "date" },
        { "name": "contact_id",  "type": "entity_link", "references": "contacts" },
        { "name": "notes",       "type": "text" },
        { "name": "is_priority", "type": "boolean", "defaultValue": "false" }
      ]
    }
  ]
}`} />
                </section>

                {/* Schema Migration */}
                <section className="flex flex-col gap-4" id="schema-migration">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Schema Auto-Migration</h2>
                    <p className="text-muted-foreground leading-7">
                        Every time you deploy a new manifest version, the runtime runs the <strong className="text-foreground font-medium">Schema Sync Engine</strong>. This engine introspects the live PostgreSQL schema, computes a diff against the incoming manifest, and generates the minimal set of DDL statements needed to reconcile the two.
                    </p>
                    <p className="text-muted-foreground leading-7">The following operations are handled automatically:</p>

                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        {[
                            { op: "New entity", ddl: "CREATE TABLE" },
                            { op: "New field on existing entity", ddl: "ALTER TABLE … ADD COLUMN" },
                            { op: "Changed NOT NULL constraint", ddl: "ALTER TABLE … ALTER COLUMN … SET/DROP NOT NULL" },
                            { op: "Changed DEFAULT value", ddl: "ALTER TABLE … ALTER COLUMN … SET DEFAULT" },
                            { op: "Changed enum values", ddl: "ALTER TABLE … DROP/ADD CONSTRAINT" },
                            { op: "New entity_link (foreign key)", ddl: "ALTER TABLE … ADD CONSTRAINT … FOREIGN KEY" },
                        ].map(({ op, ddl }) => (
                            <div key={op} className="rounded-lg border border-border bg-white/[0.02] p-3">
                                <p className="text-sm font-medium text-foreground">{op}</p>
                                <p className="mt-1 font-mono text-xs text-muted-foreground">{ddl}</p>
                            </div>
                        ))}
                    </div>

                    <Callout variant="warning" title="Destructive changes are not automatic">
                        The schema sync engine never drops tables or columns automatically. Removing a field from the manifest leaves the column in the database. This is intentional — to protect against accidental data loss. If you need to remove a column, run the DDL manually and ensure data has been migrated.
                    </Callout>

                    <p className="text-muted-foreground leading-7">
                        After a successful sync, the runtime updates the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system.apps</code> table with the new manifest version and records a deployment event in the audit log.
                    </p>
                </section>

                {/* Related Pages */}
                <section className="flex flex-col gap-4" id="related">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Related Pages</h2>
                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        {[
                            { href: "/concepts/data-contract", title: "Data Contract", desc: "Deep dive into entity definitions, field types, and schema sync behavior." },
                            { href: "/concepts/permissions", title: "Roles & Permissions", desc: "How the RBAC engine evaluates policies and enforces row-level ownership." },
                            { href: "/concepts/runtime", title: "Engine & Runtime", desc: "How the runtime boots, manages connections, and serves the HTTP API." },
                            { href: "/studio/forge", title: "Deployment — Forge", desc: "How to deploy a manifest using the Studio IDE's Forge panel." },
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

                <PageNav href="/concepts/manifest" />
            </div>
        </DocsLayout>
    );
}
