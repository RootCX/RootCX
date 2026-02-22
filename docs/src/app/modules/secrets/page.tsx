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
  { id: "master-key", title: "Master Key" },
  { id: "set-secret", title: "Set a Secret" },
  { id: "list-secrets", title: "List Secrets" },
  { id: "delete-secret", title: "Delete a Secret" },
  { id: "backend-injection", title: "Backend Injection" },
];

export default function SecretsPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        {/* Breadcrumb */}
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Secret Vault</span>
        </div>

        {/* Title */}
        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Secret Vault</h1>
          <p className="text-lg text-muted-foreground leading-7">
            AES-256-GCM encrypted key-value store per application. Securely manage API keys and credentials without exposing them in your codebase.
          </p>
        </div>

        {/* Outcomes */}
        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "Zero-leak architecture: Secrets are encrypted at rest and only decrypted in-memory during Backend process launch. Values are never returned in any API response.",
              "Complete isolation: Each application has its own dedicated namespace for secrets, preventing cross-tenant leakage even on the same Core daemon.",
              "Seamless Backend injection: Decrypted secrets are automatically injected as environment variables into your Backend process, keeping your scripts clean and secure.",
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
            The Secret Vault provides a secure, application-scoped store for sensitive configuration values
            such as API keys, database URLs, and third-party credentials. All values are encrypted at rest
            using <strong className="text-foreground font-medium">AES-256-GCM</strong> and are only ever
            decrypted in-memory when a Backend process is launched.
          </p>
          <p className="text-muted-foreground leading-7">
            Each application has its own isolated secret namespace. Secrets set for one application are
            never accessible to another, even if both run on the same Core instance. Secret values are{" "}
            <strong className="text-foreground font-medium">never returned</strong> by any API endpoint — only
            the key names are listable.
          </p>
          <p className="text-muted-foreground leading-7">
            All secret management endpoints require administrator-level authentication. Use them from
            deployment scripts, the RootCX Studio UI, or administrative tooling — not from client applications.
          </p>
        </section>

        {/* Master Key */}
        <section className="flex flex-col gap-4" id="master-key">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Master Key</h2>
          <p className="text-muted-foreground leading-7">
            The 32-byte master key is the root of all secret encryption. Core resolves it in the following
            order of precedence:
          </p>
          <ol className="flex flex-col gap-1.5 text-muted-foreground text-sm leading-7 list-none pl-0">
            <li className="flex gap-2 items-start">
              <span className="text-muted-foreground/50 font-mono text-xs mt-1">1.</span>
              <span>The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_MASTER_KEY</code> environment variable (hex-encoded 32-byte value)</span>
            </li>
            <li className="flex gap-2 items-start">
              <span className="text-muted-foreground/50 font-mono text-xs mt-1">2.</span>
              <span>The file at <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">{"<data_dir>"}/config/master.key</code></span>
            </li>
            <li className="flex gap-2 items-start">
              <span className="text-muted-foreground/50 font-mono text-xs mt-1">3.</span>
              <span>Auto-generated on first boot: a cryptographically random key is created and written to the file above</span>
            </li>
          </ol>

          <Callout variant="warning" title="Back Up Your Master Key">
            If the master key is lost, all encrypted secrets are permanently unrecoverable. Store your
            master key in a secure external secrets manager and never commit it to version control.
          </Callout>
        </section>

        {/* Set a Secret */}
        <section className="flex flex-col gap-4" id="set-secret">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Set a Secret</h2>
          <p className="text-muted-foreground leading-7">
            Creates or updates a secret for the specified application. If a secret with the given key
            already exists, its value is overwritten atomically.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/secrets
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">Request Body</h3>
          <PropertiesTable
            properties={[
              {
                name: "key",
                type: "string",
                required: true,
                description: "The secret key name. Used as the environment variable name when injected into Backend processes.",
              },
              {
                name: "value",
                type: "string",
                required: true,
                description: "The secret value to encrypt and store. Any UTF-8 string.",
              },
            ]}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X POST http://localhost:9100/api/v1/apps/my-app/secrets \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "key": "STRIPE_SECRET_KEY",
    "value": "sk_live_51H..."
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "message": "secret 'STRIPE_SECRET_KEY' set"
}`}
          />
        </section>

        {/* List Secrets */}
        <section className="flex flex-col gap-4" id="list-secrets">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">List Secrets</h2>
          <p className="text-muted-foreground leading-7">
            Returns the names of all secrets configured for the application, sorted alphabetically.
            Secret values are <strong className="text-foreground font-medium">never</strong> included — only
            the key names are returned.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/secrets
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">Example</h3>
          <CodeBlock
            language="bash"
            code={`curl http://localhost:9100/api/v1/apps/my-app/secrets \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`["EXTERNAL_DB_URL", "SENDGRID_API_KEY", "STRIPE_SECRET_KEY"]`}
          />
        </section>

        {/* Delete a Secret */}
        <section className="flex flex-col gap-4" id="delete-secret">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Delete a Secret</h2>
          <p className="text-muted-foreground leading-7">
            Permanently removes a secret from the vault. Any Backend process currently running with this
            secret will retain it until the next restart — deletion only affects future launches.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-red-400 bg-red-400/10 rounded px-2 py-0.5">DELETE</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/secrets/{"{key}"}
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X DELETE http://localhost:9100/api/v1/apps/my-app/secrets/STRIPE_SECRET_KEY \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "message": "secret 'STRIPE_SECRET_KEY' deleted"
}`}
          />
          <p className="text-muted-foreground leading-7">
            Returns <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">404 Not Found</code> if
            the key does not exist for this application.
          </p>
        </section>

        {/* Backend Injection */}
        <section className="flex flex-col gap-4" id="backend-injection">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Backend Injection</h2>
          <p className="text-muted-foreground leading-7">
            When Core launches a Backend process for an application, it fetches all secrets for that
            application, decrypts them in-memory, and injects them as environment variables into the
            spawned process. The decrypted values are discarded from Core&apos;s memory after the process starts.
          </p>

          <CodeBlock
            language="javascript"
            code={`// backend.js — secrets are available as standard env vars
export default async function handler(req) {
  const stripe = new Stripe(process.env.STRIPE_SECRET_KEY);
  const dbUrl = process.env.EXTERNAL_DB_URL;

  const charge = await stripe.charges.create({
    amount: 2000,
    currency: "usd",
    source: req.body.token,
  });

  return { success: true, chargeId: charge.id };
}`}
          />

          <Callout variant="info" title="Environment Variable Precedence">
            Vault secrets are merged on top of system-level environment variables. If both define the
            same key, the Vault secret takes precedence.
          </Callout>
        </section>

        <PageNav href="/modules/secrets" />
      </div>
    </DocsLayout>
  );
}
