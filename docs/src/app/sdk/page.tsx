import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "installation", title: "Installation" },
    { id: "use-auth", title: "useAuth" },
    { id: "use-app-collection", title: "useAppCollection" },
    { id: "use-app-record", title: "useAppRecord" },
    { id: "use-permissions", title: "usePermissions" },
    { id: "use-runtime-status", title: "useRuntimeStatus" },
    { id: "direct-client", title: "Direct HTTP client" },
    { id: "typescript-types", title: "TypeScript types" },
];

export default function SdkPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/api-reference" className="hover:text-foreground transition-colors">API Reference</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">React SDK</span>
                </div>

                <header className="flex flex-col gap-4">
                    <div className="inline-flex items-center rounded-full border border-blue-500/20 bg-blue-500/5 px-3 py-1 text-xs font-medium text-blue-400 w-fit font-mono">
                        @rootcx/runtime
                    </div>
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">React SDK</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Official React hooks for interacting with the RootCX runtime from your frontend applications.
                    </p>
                </header>

                <section className="flex flex-col gap-4" id="installation">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Installation</h2>
                    <CodeBlock language="bash" code={`# npm
npm install @rootcx/runtime

# pnpm
pnpm add @rootcx/runtime

# yarn
yarn add @rootcx/runtime`} />
                    <p className="text-muted-foreground leading-7">
                        The SDK requires React 18+ and TypeScript 5+ (recommended). It is ESM-only and works with Next.js, Vite, and other modern bundlers.
                    </p>
                    <Callout variant="info" title="Runtime URL">
                        The SDK connects to the Core daemon at <code>http://localhost:9100</code> by default. In production, point it to wherever your daemon is running.
                    </Callout>
                    <CodeBlock language="typescript" filename="src/main.tsx" code={`import { RootCXProvider } from "@rootcx/runtime";
import App from "./App";

createRoot(document.getElementById("root")!).render(
  <RootCXProvider runtimeUrl="http://localhost:9100">
    <App />
  </RootCXProvider>
);`} />
                </section>

                <section className="flex flex-col gap-4" id="use-auth">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">useAuth</h2>
                    <p className="text-muted-foreground leading-7">
                        Manage authentication state — login, registration, logout, and the current user.
                    </p>
                    <CodeBlock language="typescript" code={`import { useAuth } from "@rootcx/runtime";

const {
  user,            // AuthUser | null — current user, null if not authenticated
  isAuthenticated, // boolean
  isLoading,       // boolean — true while checking session or performing auth
  error,           // string | null
  authenticate,    // (username: string, password: string) => Promise<void>
  register,        // (opts: RegisterOptions) => Promise<void>
  logout,          // () => Promise<void>
} = useAuth();`} />
                    <CodeBlock language="tsx" filename="src/LoginPage.tsx" code={`import { useAuth } from "@rootcx/runtime";
import { useState } from "react";

export function LoginPage() {
  const { authenticate, isLoading, error } = useAuth();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    await authenticate(username, password);
    // On success, user state updates and isAuthenticated becomes true
  };

  return (
    <form onSubmit={handleSubmit}>
      <input value={username} onChange={e => setUsername(e.target.value)} />
      <input type="password" value={password} onChange={e => setPassword(e.target.value)} />
      <button type="submit" disabled={isLoading}>
        {isLoading ? "Signing in..." : "Sign in"}
      </button>
      {error && <p className="text-red-500">{error}</p>}
    </form>
  );
}`} />
                </section>

                <section className="flex flex-col gap-4" id="use-app-collection">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">useAppCollection</h2>
                    <p className="text-muted-foreground leading-7">
                        Fetch and mutate a collection of records for a given entity. Data is loaded on mount and re-fetched after mutations.
                    </p>
                    <CodeBlock language="typescript" code={`import { useAppCollection } from "@rootcx/runtime";

const {
  records,   // T[] — array of records
  isLoading, // boolean
  error,     // string | null
  create,    // (data: Partial<T>) => Promise<T>
  update,    // (id: string, data: Partial<T>) => Promise<T>
  remove,    // (id: string) => Promise<void>
  refresh,   // () => Promise<void> — manually re-fetch
} = useAppCollection<T>("appId", "entityName");`} />
                    <CodeBlock language="tsx" filename="src/ContactsList.tsx" code={`import { useAppCollection } from "@rootcx/runtime";

type Contact = {
  id: string;
  firstName: string;
  lastName: string;
  email: string;
  created_at: string;
};

export function ContactsList() {
  const { records, isLoading, error, create, remove } = useAppCollection<Contact>(
    "crm",
    "contacts"
  );

  const addContact = async () => {
    await create({
      firstName: "Bob",
      lastName: "Smith",
      email: "bob@example.com",
    });
    // records array updates automatically
  };

  if (isLoading) return <div>Loading...</div>;
  if (error) return <div>Error: {error}</div>;

  return (
    <div>
      <button onClick={addContact}>Add Contact</button>
      <ul>
        {records.map(c => (
          <li key={c.id}>
            {c.firstName} {c.lastName} — {c.email}
            <button onClick={() => remove(c.id)}>Delete</button>
          </li>
        ))}
      </ul>
    </div>
  );
}`} />
                </section>

                <section className="flex flex-col gap-4" id="use-app-record">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">useAppRecord</h2>
                    <p className="text-muted-foreground leading-7">
                        Fetch and manage a single record by ID.
                    </p>
                    <CodeBlock language="typescript" code={`import { useAppRecord } from "@rootcx/runtime";

const {
  record,    // T | null
  isLoading, // boolean
  error,     // string | null
  update,    // (data: Partial<T>) => Promise<T>
  remove,    // () => Promise<void>
} = useAppRecord<T>("appId", "entityName", recordId);`} />
                    <CodeBlock language="tsx" filename="src/ContactDetail.tsx" code={`import { useAppRecord } from "@rootcx/runtime";

export function ContactDetail({ id }: { id: string }) {
  const { record, isLoading, update } = useAppRecord<Contact>(
    "crm", "contacts", id
  );

  if (isLoading) return <Spinner />;
  if (!record) return <div>Not found</div>;

  return (
    <div>
      <h1>{record.firstName} {record.lastName}</h1>
      <button onClick={() => update({ email: "new@email.com" })}>
        Update email
      </button>
    </div>
  );
}`} />
                </section>

                <section className="flex flex-col gap-4" id="use-permissions">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">usePermissions</h2>
                    <p className="text-muted-foreground leading-7">
                        Check the current user's RBAC permissions for a given app. Use to conditionally show/hide UI elements.
                    </p>
                    <CodeBlock language="typescript" code={`import { usePermissions } from "@rootcx/runtime";

const {
  permissions, // Record<entity, { actions: string[], ownership: boolean }>
  can,         // (entity: string, action: string) => boolean
  isLoading,
} = usePermissions("appId");`} />
                    <CodeBlock language="tsx" code={`import { usePermissions } from "@rootcx/runtime";

export function ContactActions({ contactId }: { contactId: string }) {
  const { can } = usePermissions("crm");

  return (
    <div>
      {can("contacts", "update") && (
        <button>Edit</button>
      )}
      {can("contacts", "delete") && (
        <button className="text-red-500">Delete</button>
      )}
    </div>
  );
}`} />
                    <Callout variant="warning" title="Client-side only">
                        <code>usePermissions</code> is for UI convenience only. Never rely on client-side permission checks for security. The Core daemon enforces RBAC on every request server-side.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="use-runtime-status">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">useRuntimeStatus</h2>
                    <p className="text-muted-foreground leading-7">
                        Monitor the health of the Core daemon. Useful for showing connection status in your app.
                    </p>
                    <CodeBlock language="typescript" code={`import { useRuntimeStatus } from "@rootcx/runtime";

const {
  status,      // "ok" | "degraded" | "offline" | null
  isConnected, // boolean
  isLoading,   // boolean
  postgres,    // "running" | "stopped" | null
} = useRuntimeStatus();`} />
                    <CodeBlock language="tsx" code={`function StatusBadge() {
  const { isConnected } = useRuntimeStatus();

  return (
    <div className="flex items-center gap-1.5">
      <span className={\`h-2 w-2 rounded-full \${isConnected ? "bg-green-500" : "bg-red-500"}\`} />
      <span className="text-xs">{isConnected ? "Connected" : "Offline"}</span>
    </div>
  );
}`} />
                </section>

                <section className="flex flex-col gap-4" id="direct-client">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Direct HTTP client</h2>
                    <p className="text-muted-foreground leading-7">
                        For use cases not covered by the hooks, import the underlying HTTP client directly:
                    </p>
                    <CodeBlock language="typescript" code={`import { RuntimeClient } from "@rootcx/runtime";

const client = new RuntimeClient("http://localhost:9100");

// Authenticate
await client.authenticate("alice", "password");

// CRUD
const contacts = await client.listRecords("crm", "contacts");
const created = await client.createRecord("crm", "contacts", {
  firstName: "Alice",
  email: "alice@example.com",
});

// RPC
const result = await client.rpc("crm", "sendWelcomeEmail", {
  contactId: created.id,
});

// Jobs
const { job_id } = await client.enqueueJob("crm", {
  type: "generate_report",
  period: "monthly",
});`} />
                </section>

                <section className="flex flex-col gap-4" id="typescript-types">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">TypeScript types</h2>
                    <p className="text-muted-foreground leading-7">
                        Key types exported from <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">@rootcx/runtime</code>:
                    </p>
                    <CodeBlock language="typescript" code={`// Authenticated user
export interface AuthUser {
  id: string;
  username: string;
  email: string | null;
  displayName: string | null;
}

// Runtime status
export interface RuntimeStatus {
  postgres: "running" | "stopped";
  runtime: "ok" | "error";
  forge: { app_id: string | null; status: string } | null;
}

// Base record fields (all entity records include these)
export interface BaseRecord {
  id: string;
  owner_id?: string;
  created_at: string;
  updated_at: string;
}

// Permission grant for an entity
export interface EntityPermission {
  actions: Array<"create" | "read" | "update" | "delete">;
  ownership: boolean;
}

// Map of entity → permissions
export type PermissionsMap = Record<string, EntityPermission>;

// Registration options
export interface RegisterOptions {
  username: string;
  password: string;
  email?: string;
  displayName?: string;
}`} />
                </section>

                <PageNav href="/sdk" />
            </div>
        </DocsLayout>
    );
}
