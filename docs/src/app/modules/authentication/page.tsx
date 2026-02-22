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
  { id: "auth-modes", title: "Auth Modes" },
  { id: "registration", title: "Registration" },
  { id: "login", title: "Login" },
  { id: "tokens", title: "Token Structure" },
  { id: "refresh", title: "Refresh" },
  { id: "logout", title: "Logout" },
  { id: "current-user", title: "Current User" },
  { id: "security", title: "Security" },
];

export default function AuthenticationPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        {/* Breadcrumb */}
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Authentication</span>
        </div>

        {/* Title */}
        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Authentication</h1>
          <p className="text-lg text-muted-foreground leading-7">
            Zero-config JWT authentication with Argon2id password hashing, short-lived access tokens, and server-side session management.
          </p>
        </div>

        {/* Outcomes */}
        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "Zero external dependencies: Complete authentication layer without relying on third-party identity providers.",
              "Instant API security: Secure all generated endpoints and custom Backend logic via native JWT integration.",
              "Perfect RBAC integration: Every user is automatically mapped to strictly enforced roles and permissions."
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
            RootCX ships a complete authentication system built into Core. Every application gets
            user registration, password-based login with Argon2id hashing, short-lived access tokens (15 minutes), long-lived
            refresh tokens (30 days), and server-side session invalidation — all stored in the same PostgreSQL database
            as your application data.
          </p>
          <p className="text-muted-foreground leading-7">
            Authentication integrates directly with the{" "}
            <Link href="/modules/rbac" className="text-foreground underline underline-offset-4 hover:text-muted-foreground transition-colors">RBAC module</Link>.
            The first non-system user registered is automatically promoted to admin on all RBAC-enabled apps.
            Subsequent users receive the default role configured in the manifest.
          </p>
          <p className="text-muted-foreground leading-7">
            All auth endpoints are served under{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">/api/v1/auth</code> and
            do not require an application ID — authentication is global to the Core instance.
          </p>
        </section>

        {/* Auth Modes */}
        <section className="flex flex-col gap-4" id="auth-modes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Auth Modes</h2>
          <p className="text-muted-foreground leading-7">
            Core supports two authentication enforcement modes, configured via the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH</code>{" "}
            environment variable.
          </p>

          <PropertiesTable
            properties={[
              {
                name: "public",
                type: "default",
                description: "Tokens are validated when present. If no Authorization header is provided, the request proceeds anonymously. Ownership-based data endpoints will return empty results for anonymous users.",
              },
              {
                name: "required",
                type: "strict",
                description: "Every request to a data or Backend endpoint must include a valid Bearer token. Unauthenticated requests are rejected with 401. The /api/v1/auth/* endpoints are always public.",
              },
            ]}
          />

          <CodeBlock
            language="bash"
            code={`# Enable strict auth for production
ROOTCX_AUTH=required rootcx-core start`}
          />

          <Callout variant="info" title="Recommended for Production">
            Set <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH=required</code> for
            all production deployments. The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">public</code> mode
            is useful during development and for apps that intentionally serve unauthenticated users.
          </Callout>
        </section>

        {/* Registration */}
        <section className="flex flex-col gap-4" id="registration">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Registration</h2>
          <p className="text-muted-foreground leading-7">
            Creates a new user account. Usernames must be unique across the Core instance. Passwords are
            hashed with Argon2id before storage. Registration does not return tokens — the client must
            call the login endpoint separately.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/auth/register
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">Request Body</h3>
          <PropertiesTable
            properties={[
              {
                name: "username",
                type: "string",
                required: true,
                description: "Must be between 3 and 64 characters. Alphanumeric, underscores, and hyphens. Case-insensitive uniqueness is enforced.",
              },
              {
                name: "password",
                type: "string",
                required: true,
                description: "Minimum 8 characters. Hashed with Argon2id before storage.",
              },
              {
                name: "email",
                type: "string",
                required: false,
                description: "Optional email address. Must be unique if provided.",
              },
              {
                name: "displayName",
                type: "string",
                required: false,
                description: "Optional human-readable display name, up to 128 characters.",
              },
            ]}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X POST http://localhost:9100/api/v1/auth/register \\
  -H "Content-Type: application/json" \\
  -d '{
    "username": "alice",
    "password": "supersecret123",
    "email": "alice@example.com",
    "displayName": "Alice Smith"
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 201 Created</h3>
          <CodeBlock
            language="json"
            code={`{
  "user": {
    "id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
    "username": "alice",
    "email": "alice@example.com",
    "displayName": "Alice Smith",
    "createdAt": "2025-01-15T10:30:00.000Z"
  }
}`}
          />

          <CodeBlock
            language="json"
            code={`// 409 Conflict — username already taken
{
  "error": "username already exists"
}`}
          />
        </section>

        {/* Login */}
        <section className="flex flex-col gap-4" id="login">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Login</h2>
          <p className="text-muted-foreground leading-7">
            Authenticates a user with their username and password. On success, returns a short-lived access
            token, a long-lived refresh token, and the user object. A new server-side session is
            created for every login — multiple concurrent sessions per user are supported.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/auth/login
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X POST http://localhost:9100/api/v1/auth/login \\
  -H "Content-Type: application/json" \\
  -d '{
    "username": "alice",
    "password": "supersecret123"
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "accessToken": "eyJhbGciOiJIUzI1NiJ9...",
  "refreshToken": "eyJhbGciOiJIUzI1NiJ9...",
  "expiresIn": 900,
  "user": {
    "id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
    "username": "alice",
    "email": "alice@example.com",
    "displayName": "Alice Smith",
    "createdAt": "2025-01-15T10:30:00.000Z"
  }
}`}
          />

          <CodeBlock
            language="json"
            code={`// 401 Unauthorized — wrong username or password
{
  "error": "invalid credentials"
}`}
          />
        </section>

        {/* Token Structure */}
        <section className="flex flex-col gap-4" id="tokens">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Token Structure</h2>
          <p className="text-muted-foreground leading-7">
            Both tokens are HS256-signed JWTs. The access token is valid for{" "}
            <strong className="text-foreground font-medium">15 minutes</strong> and contains identity claims.
            The refresh token is valid for <strong className="text-foreground font-medium">30 days</strong> and
            contains a session reference for server-side invalidation.
          </p>

          <h3 className="text-lg font-semibold text-foreground mt-2">Access Token Payload</h3>
          <CodeBlock
            language="json"
            code={`{
  "userId": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "username": "alice",
  "role": "user",
  "iat": 1736936200,
  "exp": 1736937100
}`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Using Tokens</h3>
          <p className="text-muted-foreground leading-7">
            Include the access token in the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Authorization</code>{" "}
            header of every authenticated request:
          </p>
          <CodeBlock
            language="bash"
            code={`curl http://localhost:9100/api/v1/apps/my-app/collections/posts \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <Callout variant="warning" title="Store Tokens Securely">
            Never store tokens in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">localStorage</code> in
            browser environments — use <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">httpOnly</code> cookies
            or secure in-memory storage instead.
          </Callout>
        </section>

        {/* Refresh */}
        <section className="flex flex-col gap-4" id="refresh">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Refresh</h2>
          <p className="text-muted-foreground leading-7">
            Exchanges a valid refresh token for a new short-lived access token. The refresh token itself
            is not rotated — it remains valid until logout or expiry.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/auth/refresh
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X POST http://localhost:9100/api/v1/auth/refresh \\
  -H "Content-Type: application/json" \\
  -d '{
    "refreshToken": "eyJhbGciOiJIUzI1NiJ9..."
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "accessToken": "eyJhbGciOiJIUzI1NiJ9...",
  "expiresIn": 900
}`}
          />

          <CodeBlock
            language="json"
            code={`// 401 — refresh token invalid or session revoked
{
  "error": "invalid or expired refresh token"
}`}
          />
        </section>

        {/* Logout */}
        <section className="flex flex-col gap-4" id="logout">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Logout</h2>
          <p className="text-muted-foreground leading-7">
            Invalidates the current session on the server. After logout, any refresh token associated with
            the session will be rejected. Access tokens issued before logout remain cryptographically valid
            until their 15-minute expiry — clients should discard them immediately.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/auth/logout
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl -X POST http://localhost:9100/api/v1/auth/logout \\
  -H "Content-Type: application/json" \\
  -d '{
    "refreshToken": "eyJhbGciOiJIUzI1NiJ9..."
  }'`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "message": "logged out"
}`}
          />
        </section>

        {/* Current User */}
        <section className="flex flex-col gap-4" id="current-user">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Current User</h2>
          <p className="text-muted-foreground leading-7">
            Returns the authenticated user's profile from the database, ensuring the latest role and
            profile data is returned.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/auth/me
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock
            language="bash"
            code={`curl http://localhost:9100/api/v1/auth/me \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
          />

          <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
          <CodeBlock
            language="json"
            code={`{
  "id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "username": "alice",
  "email": "alice@example.com",
  "displayName": "Alice Smith",
  "createdAt": "2025-01-15T10:30:00.000Z"
}`}
          />
        </section>

        {/* Security */}
        <section className="flex flex-col gap-4" id="security">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Security</h2>

          <h3 className="text-lg font-semibold text-foreground mt-2">Password Hashing</h3>
          <p className="text-muted-foreground leading-7">
            All passwords are hashed using <strong className="text-foreground font-medium">Argon2id</strong>,
            the OWASP-recommended algorithm. RootCX uses the Rust{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">argon2</code> crate
            with its default parameters (OWASP-recommended settings).
          </p>

          <h3 className="text-lg font-semibold text-foreground mt-2">JWT Secret</h3>
          <p className="text-muted-foreground leading-7">
            The JWT secret used to sign all tokens is resolved from:
          </p>
          <ol className="flex flex-col gap-1.5 text-muted-foreground text-sm leading-7 list-none pl-0">
            <li className="flex gap-2 items-start">
              <span className="text-muted-foreground/50 font-mono text-xs mt-1">1.</span>
              <span>The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_JWT_SECRET</code> environment variable</span>
            </li>
            <li className="flex gap-2 items-start">
              <span className="text-muted-foreground/50 font-mono text-xs mt-1">2.</span>
              <span>The file <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">{"{data_dir}"}/config/jwt.key</code></span>
            </li>
            <li className="flex gap-2 items-start">
              <span className="text-muted-foreground/50 font-mono text-xs mt-1">3.</span>
              <span>Auto-generated on first boot and written to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">config/jwt.key</code></span>
            </li>
          </ol>
          <Callout variant="warning" title="Persist Your JWT Secret">
            If the JWT secret changes, all existing tokens are invalidated and all users will be logged out.
            Always set <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_JWT_SECRET</code> explicitly
            in production, or mount <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">config/jwt.key</code> as
            a persistent volume.
          </Callout>
        </section>

        <PageNav href="/modules/authentication" />
      </div>
    </DocsLayout>
  );
}
