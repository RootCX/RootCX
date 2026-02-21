import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const tocItems = [
  { id: "overview", title: "Overview" },
  { id: "how-encryption-works", title: "How Encryption Works" },
  { id: "set-secret", title: "Set a Secret" },
  { id: "list-secrets", title: "List Secrets" },
  { id: "delete-secret", title: "Delete a Secret" },
  { id: "worker-injection", title: "Worker Injection" },
  { id: "database-schema", title: "Database Schema" },
  { id: "best-practices", title: "Best Practices" },
];

export default function SecretsPage() {
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
            <span className="text-foreground">Secret Vault</span>
          </div>

          {/* Title */}
          <div className="flex flex-col gap-3">
            <h1 className="text-4xl font-bold tracking-tight">Secret Vault</h1>
            <p className="text-lg text-muted-foreground leading-7">
              AES-256-GCM encrypted key-value store per application. Store API keys, database URLs, and
              third-party credentials securely — secrets are decrypted only when workers need them and
              are never returned in API responses.
            </p>
          </div>

          {/* Overview */}
          <section className="flex flex-col gap-4" id="overview">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
            <p className="text-muted-foreground leading-7">
              The Secret Vault provides a secure, application-scoped store for sensitive configuration values.
              Unlike environment variables baked into container images or config files checked into version
              control, secrets managed by RootCX are encrypted at rest in PostgreSQL using AES-256-GCM and
              are only decrypted in-memory, on demand, when a Worker process starts.
            </p>
            <p className="text-muted-foreground leading-7">
              Each application has its own isolated secret namespace. Secrets set for{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">app-a</code> are
              never accessible to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">app-b</code>,
              even if both run on the same runtime instance. Secret values are{" "}
              <strong className="text-foreground font-medium">never returned</strong> by any API endpoint — only
              the key names are listable.
            </p>
            <p className="text-muted-foreground leading-7">
              All secret management API endpoints require administrator-level authentication. They are not
              intended to be called from client applications — use them from CI/CD pipelines, deployment
              scripts, or the RootCX Studio UI.
            </p>
          </section>

          {/* How Encryption Works */}
          <section className="flex flex-col gap-4" id="how-encryption-works">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">How Encryption Works</h2>
            <p className="text-muted-foreground leading-7">
              Every secret value is encrypted with <strong className="text-foreground font-medium">AES-256-GCM</strong>{" "}
              before being written to the database. GCM (Galois/Counter Mode) provides both confidentiality
              and authenticated encryption — any tampering with the ciphertext is detected on decryption.
            </p>

            <h3 className="text-lg font-semibold text-foreground mt-2">Encryption Process</h3>
            <p className="text-muted-foreground leading-7">
              When a secret is set, the runtime performs the following steps:
            </p>
            <ol className="flex flex-col gap-2 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">1.</span>
                <span>A <strong className="text-foreground font-medium">12-byte random nonce</strong> is generated using a cryptographically secure random number generator. A fresh nonce is generated for every encryption operation, including overwrites.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">2.</span>
                <span>The secret value is encoded as UTF-8 bytes and encrypted with <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">AES-256-GCM</code> using the master key and the generated nonce.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">3.</span>
                <span>The nonce and ciphertext (including the GCM authentication tag) are stored as separate{" "}
                  <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">BYTEA</code> columns in the database.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">4.</span>
                <span>To decrypt, the runtime fetches the nonce and ciphertext, then uses AES-256-GCM decryption with the master key. If the GCM tag is invalid (tampered data or wrong key), decryption fails with an error.</span>
              </li>
            </ol>

            <h3 className="text-lg font-semibold text-foreground mt-2">Master Key</h3>
            <p className="text-muted-foreground leading-7">
              The 32-byte master key is the root of all secret encryption. It is loaded in the following
              order of precedence:
            </p>
            <ol className="flex flex-col gap-1.5 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-2 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1">1.</span>
                <span>The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_MASTER_KEY</code> environment variable — a hex-encoded 32-byte value</span>
              </li>
              <li className="flex gap-2 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1">2.</span>
                <span>The file <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/.rootcx/config/master.key</code> — binary 32-byte file</span>
              </li>
              <li className="flex gap-2 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1">3.</span>
                <span>Auto-generated: a cryptographically random 32-byte key is generated and written to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/.rootcx/config/master.key</code> on first boot</span>
              </li>
            </ol>

            <CodeBlock
              language="bash"
              code={`# Generate a master key manually (for use in ROOTCX_MASTER_KEY)
openssl rand -hex 32
# Output example:
# a3f8c2d1e4b5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1

# Set in your environment
export ROOTCX_MASTER_KEY=a3f8c2d1e4b5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1`}
            />

            <Callout variant="warning" title="Back Up Your Master Key">
              If the master key is lost, all encrypted secrets are permanently unrecoverable. The runtime
              cannot decrypt ciphertext without the original key. Always store the master key in a secure
              secrets manager (AWS Secrets Manager, HashiCorp Vault, GCP Secret Manager) and never commit
              it to version control.
            </Callout>
          </section>

          {/* Set a Secret */}
          <section className="flex flex-col gap-4" id="set-secret">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Set a Secret</h2>
            <p className="text-muted-foreground leading-7">
              Creates or updates a secret for the specified application. This operation is idempotent — if a
              secret with the given key already exists, its value is overwritten and a new nonce is generated.
              The old ciphertext is replaced atomically.
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
                  description: "The secret key name. Alphanumeric characters, underscores, and hyphens. Maximum 128 characters. Used as the environment variable name when injected into Workers.",
                },
                {
                  name: "value",
                  type: "string",
                  required: true,
                  description: "The secret value to encrypt and store. Any UTF-8 string. Maximum 64 KiB. Encrypted immediately in the request handler before the database write.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Examples</h3>
            <CodeBlock
              language="bash"
              code={`# Set an API key
curl -X POST https://your-runtime.example.com/api/v1/apps/my-app/secrets \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "key": "STRIPE_SECRET_KEY",
    "value": "sk_live_51H..."
  }'

# Set a database URL
curl -X POST https://your-runtime.example.com/api/v1/apps/my-app/secrets \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "key": "EXTERNAL_DB_URL",
    "value": "postgresql://user:password@db.example.com:5432/mydb"
  }'`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
            <CodeBlock
              language="json"
              code={`{
  "data": {
    "key": "STRIPE_SECRET_KEY",
    "created_at": "2025-01-15T10:30:00.000Z"
  }
}`}
            />
            <p className="text-muted-foreground leading-7">
              Note that the response contains only the key name and timestamp — the encrypted value is
              never echoed back.
            </p>
          </section>

          {/* List Secrets */}
          <section className="flex flex-col gap-4" id="list-secrets">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">List Secrets</h2>
            <p className="text-muted-foreground leading-7">
              Returns the names of all secrets configured for the application. Secret values are{" "}
              <strong className="text-foreground font-medium">never</strong> included in this response — only
              the key names are returned. This allows you to audit what secrets are configured without
              any risk of accidental exposure.
            </p>

            <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
              <div className="flex items-center gap-3">
                <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
                <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                  /api/v1/apps/{"{appId}"}/secrets
                </code>
              </div>
            </div>

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
            <CodeBlock
              language="bash"
              code={`curl https://your-runtime.example.com/api/v1/apps/my-app/secrets \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
            <CodeBlock
              language="json"
              code={`{
  "data": [
    {
      "key": "STRIPE_SECRET_KEY",
      "created_at": "2025-01-15T10:30:00.000Z"
    },
    {
      "key": "EXTERNAL_DB_URL",
      "created_at": "2025-01-15T10:32:11.000Z"
    },
    {
      "key": "SENDGRID_API_KEY",
      "created_at": "2025-01-16T08:00:00.000Z"
    }
  ],
  "count": 3
}`}
            />
          </section>

          {/* Delete a Secret */}
          <section className="flex flex-col gap-4" id="delete-secret">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Delete a Secret</h2>
            <p className="text-muted-foreground leading-7">
              Permanently removes a secret from the vault. The encrypted ciphertext and nonce are deleted
              from the database. Any Worker that is currently running with this secret injected will continue
              to have it in its environment until the next restart — the deletion only affects future Worker
              launches.
            </p>

            <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
              <div className="flex items-center gap-3">
                <span className="text-xs font-mono font-bold text-red-400 bg-red-400/10 rounded px-2 py-0.5">DELETE</span>
                <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                  /api/v1/apps/{"{appId}"}/secrets/{"{key}"}
                </code>
              </div>
            </div>

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
            <CodeBlock
              language="bash"
              code={`curl -X DELETE https://your-runtime.example.com/api/v1/apps/my-app/secrets/STRIPE_SECRET_KEY \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <p className="text-muted-foreground leading-7">
              Returns <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">204 No Content</code> on
              success. Returns <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">404 Not Found</code> if
              the key does not exist for this application.
            </p>
          </section>

          {/* Worker Injection */}
          <section className="flex flex-col gap-4" id="worker-injection">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Worker Injection</h2>
            <p className="text-muted-foreground leading-7">
              The primary purpose of the Secret Vault is to make secrets available to Worker processes without
              requiring them to be stored in the manifest, environment, or source code. Before the runtime
              starts a Worker process for an application, it performs the following steps:
            </p>
            <ol className="flex flex-col gap-2 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">1.</span>
                <span>All secret rows for the application are fetched from the database in a single query.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">2.</span>
                <span>Each nonce/ciphertext pair is decrypted in-memory using the master key. If any decryption fails, the Worker launch is aborted.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">3.</span>
                <span>Each decrypted secret is added to the Worker's environment as an environment variable, with the key name as the variable name.</span>
              </li>
              <li className="flex gap-3 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1 shrink-0">4.</span>
                <span>The decrypted values are discarded from runtime memory after the process is spawned — they are not held in the parent process.</span>
              </li>
            </ol>

            <h3 className="text-lg font-semibold text-foreground mt-2">Accessing Secrets in Workers</h3>
            <p className="text-muted-foreground leading-7">
              Inside your Worker code, secrets are accessible as standard environment variables:
            </p>
            <CodeBlock
              language="javascript"
              code={`// worker.js
export default async function handler(req) {
  // Secrets are available as process.env variables
  const stripe = new Stripe(process.env.STRIPE_SECRET_KEY);
  const dbUrl = process.env.EXTERNAL_DB_URL;

  // Use them as you would any environment variable
  const charge = await stripe.charges.create({
    amount: 2000,
    currency: "usd",
    source: req.body.token,
  });

  return { success: true, chargeId: charge.id };
}`}
            />

            <Callout variant="info" title="Secrets and Environment Variables">
              Runtime-level environment variables (set when launching the RootCX process itself) are also
              available to Workers. Secrets set via the Vault are merged on top — Vault secrets take
              precedence over process-level environment variables of the same name.
            </Callout>
          </section>

          {/* Database Schema */}
          <section className="flex flex-col gap-4" id="database-schema">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Database Schema</h2>
            <p className="text-muted-foreground leading-7">
              The Secret Vault uses a single table in the runtime's PostgreSQL database. The schema is
              provisioned automatically on first boot.
            </p>
            <CodeBlock
              language="sql"
              code={`CREATE TABLE IF NOT EXISTS app_secrets (
  app_id      TEXT NOT NULL,
  key_name    TEXT NOT NULL,
  nonce       BYTEA NOT NULL,
  ciphertext  BYTEA NOT NULL,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),

  PRIMARY KEY (app_id, key_name)
);

CREATE INDEX IF NOT EXISTS app_secrets_app_id_idx
  ON app_secrets (app_id);`}
            />

            <p className="text-muted-foreground leading-7">
              The composite primary key on <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">(app_id, key_name)</code> ensures
              that key names are unique per application and enables efficient lookups by application when
              loading all secrets before a Worker launch.
            </p>
            <p className="text-muted-foreground leading-7">
              Note that the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">nonce</code> and{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ciphertext</code> columns
              store raw binary data as <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">BYTEA</code>.
              The nonce is always 12 bytes (96 bits, the standard GCM nonce size) and the ciphertext length
              equals the plaintext length plus 16 bytes for the GCM authentication tag.
            </p>
          </section>

          {/* Best Practices */}
          <section className="flex flex-col gap-4" id="best-practices">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Best Practices</h2>

            <h3 className="text-lg font-semibold text-foreground mt-2">Rotate Secrets Regularly</h3>
            <p className="text-muted-foreground leading-7">
              To rotate a secret, simply call the Set Secret endpoint with the same key and a new value.
              The operation is atomic — the old ciphertext is replaced and a new nonce is generated in a
              single database upsert. Restart affected Workers after rotation so they pick up the new value.
            </p>
            <CodeBlock
              language="bash"
              code={`# Rotate the Stripe key
curl -X POST https://your-runtime.example.com/api/v1/apps/my-app/secrets \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..." \\
  -H "Content-Type: application/json" \\
  -d '{
    "key": "STRIPE_SECRET_KEY",
    "value": "sk_live_new_key_value_here"
  }'`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">What to Store in the Vault</h3>
            <p className="text-muted-foreground leading-7">
              The Secret Vault is the right place for any value that:
            </p>
            <ul className="flex flex-col gap-1.5 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Would cause a security incident if exposed (API keys, OAuth client secrets, signing keys)</span>
              </li>
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Differs between deployment environments (dev/staging/prod database URLs)</span>
              </li>
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Needs to be rotated without redeploying the application</span>
              </li>
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Should not appear in logs, error messages, or version control</span>
              </li>
            </ul>

            <h3 className="text-lg font-semibold text-foreground mt-2">Set Secrets from CI/CD</h3>
            <CodeBlock
              language="yaml"
              code={[
                "# GitHub Actions — set secrets during deployment",
                "- name: Configure application secrets",
                "  run: |",
                "    curl -X POST $ROOTCX_URL/api/v1/apps/my-app/secrets \\",
                '      -H "Authorization: Bearer $ROOTCX_ADMIN_TOKEN" \\',
                '      -H "Content-Type: application/json" \\',
                '      -d \'{"key":"STRIPE_SECRET_KEY","value":"$STRIPE_SECRET_KEY"}\'',
              ].join("\n")}
            />

            <Callout variant="warning" title="Never Store in Manifest">
              Do not put secret values directly in your <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">manifest.yaml</code> or
              any other file committed to version control. Even private repositories get leaked. The Secret
              Vault exists precisely to avoid this pattern.
            </Callout>

            <h3 className="text-lg font-semibold text-foreground mt-2">Master Key Management</h3>
            <p className="text-muted-foreground leading-7">
              Treat the master key with the same level of security as a root password. Recommended approaches:
            </p>
            <ul className="flex flex-col gap-1.5 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Store in <strong className="text-foreground font-medium">AWS Secrets Manager</strong> and fetch at container startup via instance role</span>
              </li>
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Use <strong className="text-foreground font-medium">Kubernetes Secrets</strong> with encryption at rest enabled and mount as environment variable</span>
              </li>
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Use <strong className="text-foreground font-medium">HashiCorp Vault</strong> with AppRole authentication to inject at startup</span>
              </li>
              <li className="flex gap-2">
                <span className="text-muted-foreground/50 mt-0.5">—</span>
                <span>Never store in the same database as the encrypted secrets</span>
              </li>
            </ul>
          </section>

        </div>

        <PageNav href="/modules/secrets" />
      </div>
    </DocsLayout>
  );
}
