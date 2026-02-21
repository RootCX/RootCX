import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const tocItems = [
  { id: "overview", title: "Overview" },
  { id: "auth-modes", title: "Auth Modes" },
  { id: "registration", title: "Registration" },
  { id: "login", title: "Login" },
  { id: "tokens", title: "Token Structure" },
  { id: "refresh", title: "Refresh Token" },
  { id: "logout", title: "Logout" },
  { id: "current-user", title: "Current User" },
  { id: "security", title: "Security" },
];

export default function AuthenticationPage() {
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
            <span className="text-foreground">Authentication</span>
          </div>

          {/* Title */}
          <div className="flex flex-col gap-3">
            <h1 className="text-4xl font-bold tracking-tight">Authentication</h1>
            <p className="text-lg text-muted-foreground leading-7">
              Native JWT-based authentication built into every RootCX runtime. Registration, login, token refresh,
              and session management are available out of the box with no external auth service required.
            </p>
          </div>

          {/* Overview */}
          <section className="flex flex-col gap-4" id="overview">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
            <p className="text-muted-foreground leading-7">
              RootCX ships a complete authentication system as part of the core runtime. Every application gets
              user registration, password-based login with Argon2id hashing, short-lived access tokens, long-lived
              refresh tokens, and server-side session invalidation — all stored in the same PostgreSQL database
              as your application data.
            </p>
            <p className="text-muted-foreground leading-7">
              Authentication integrates directly with the{" "}
              <Link href="/modules/rbac" className="text-foreground underline underline-offset-4 hover:text-muted-foreground transition-colors">RBAC module</Link>{" "}
              — every registered user is assigned a default role, and role assignments are included in the JWT
              payload so authorization decisions can be made without additional database lookups on each request.
            </p>
            <p className="text-muted-foreground leading-7">
              All auth endpoints are served under the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">/api/v1/auth</code> prefix
              and do not require an application ID in the path — authentication is global to the runtime instance.
            </p>
          </section>

          {/* Auth Modes */}
          <section className="flex flex-col gap-4" id="auth-modes">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Auth Modes</h2>
            <p className="text-muted-foreground leading-7">
              The runtime supports two authentication enforcement modes, configured via the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH</code>{" "}
              environment variable or the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">auth.mode</code> config key.
            </p>

            <PropertiesTable
              properties={[
                {
                  name: "public",
                  type: "default",
                  description: "Tokens are validated when present. If no Authorization header is provided, the request proceeds anonymously. Ownership-based data endpoints will return empty results for anonymous users rather than rejecting the request.",
                },
                {
                  name: "required",
                  type: "strict",
                  description: "Every request to a data or worker endpoint must include a valid Bearer token. Unauthenticated requests are rejected with 401 Unauthorized before reaching any handler. The /api/v1/auth/* endpoints themselves are always public.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Setting Auth Mode</h3>
            <CodeBlock
              language="bash"
              code={`# Via environment variable
ROOTCX_AUTH=required rootcx start

# Via config file (config/rootcx.yaml)
auth:
  mode: required`}
            />

            <Callout variant="info" title="Recommended for Production">
              Set <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH=required</code> for
              all production deployments. The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">public</code> mode
              is useful during development and for applications that intentionally serve unauthenticated users (e.g. public read APIs).
            </Callout>
          </section>

          {/* Registration */}
          <section className="flex flex-col gap-4" id="registration">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Registration</h2>
            <p className="text-muted-foreground leading-7">
              Creates a new user account. Usernames must be unique across the runtime instance. Passwords are
              hashed with Argon2id before storage — plaintext passwords are never persisted.
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
                  description: "Must be between 3 and 64 characters. Alphanumeric, underscores, and hyphens are allowed. Case-insensitive uniqueness is enforced.",
                },
                {
                  name: "password",
                  type: "string",
                  required: true,
                  description: "Minimum 8 characters. No maximum length enforced by the runtime, though your client UI should apply reasonable limits. Hashed with Argon2id before storage.",
                },
                {
                  name: "email",
                  type: "string",
                  required: false,
                  description: "Optional email address. Stored as-is, not validated for deliverability. Must be unique if provided.",
                },
                {
                  name: "displayName",
                  type: "string",
                  required: false,
                  description: "Optional human-readable display name, up to 128 characters. Defaults to the username if not provided.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
            <CodeBlock
              language="bash"
              code={`curl -X POST https://your-runtime.example.com/api/v1/auth/register \\
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
  "data": {
    "user": {
      "id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
      "username": "alice",
      "email": "alice@example.com",
      "displayName": "Alice Smith",
      "role": "user",
      "created_at": "2025-01-15T10:30:00.000Z"
    },
    "access_token": "eyJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiIwMTkyNmIzZS0xMjM0LTcwMDAtYWFhYS1iYmJiY2NjY2RkZGQiLCJ1c2VybmFtZSI6ImFsaWNlIiwicm9sZSI6InVzZXIiLCJpYXQiOjE3MzY5MzYyMDAsImV4cCI6MTczNjkzNzEwMH0.SIGNATURE",
    "refresh_token": "eyJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiIwMTkyNmIzZS0xMjM0LTcwMDAtYWFhYS1iYmJiY2NjY2RkZGQiLCJzZXNzaW9uSWQiOiJzZXNzaW9uLXV1aWQiLCJpYXQiOjE3MzY5MzYyMDAsImV4cCI6MTczOTUyODIwMH0.SIGNATURE"
  }
}`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Validation Errors</h3>
            <CodeBlock
              language="json"
              code={`// 409 Conflict — username already taken
{
  "error": "username already exists"
}

// 400 Bad Request — password too short
{
  "error": "validation failed",
  "details": {
    "field": "password",
    "message": "password must be at least 8 characters"
  }
}`}
            />
          </section>

          {/* Login */}
          <section className="flex flex-col gap-4" id="login">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Login</h2>
            <p className="text-muted-foreground leading-7">
              Authenticates a user with their username and password. On success, returns a short-lived access
              token, a long-lived refresh token, and the full user object. A new server-side session record is
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

            <h3 className="text-lg font-semibold text-foreground mt-2">Request Body</h3>
            <PropertiesTable
              properties={[
                {
                  name: "username",
                  type: "string",
                  required: true,
                  description: "The user's registered username. Case-insensitive.",
                },
                {
                  name: "password",
                  type: "string",
                  required: true,
                  description: "The user's plaintext password. Verified against the Argon2id hash stored in the database.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
            <CodeBlock
              language="bash"
              code={`curl -X POST https://your-runtime.example.com/api/v1/auth/login \\
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
  "data": {
    "access_token": "eyJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiIwMTkyNmIzZS0xMjM0LTcwMDAtYWFhYS1iYmJiY2NjY2RkZGQiLCJ1c2VybmFtZSI6ImFsaWNlIiwicm9sZSI6InVzZXIiLCJpYXQiOjE3MzY5MzYyMDAsImV4cCI6MTczNjkzNzEwMH0.SIGNATURE",
    "refresh_token": "eyJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiIwMTkyNmIzZS0xMjM0LTcwMDAtYWFhYS1iYmJiY2NjY2RkZGQiLCJzZXNzaW9uSWQiOiJzZXNzaW9uLXV1aWQiLCJpYXQiOjE3MzY5MzYyMDAsImV4cCI6MTczOTUyODIwMH0.SIGNATURE",
    "user": {
      "id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
      "username": "alice",
      "email": "alice@example.com",
      "displayName": "Alice Smith",
      "role": "user",
      "created_at": "2025-01-15T10:30:00.000Z"
    }
  }
}`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Failed Login</h3>
            <CodeBlock
              language="json"
              code={`// 401 Unauthorized — wrong username or password
// Note: the same message is used for both cases to prevent username enumeration
{
  "error": "invalid credentials"
}`}
            />
          </section>

          {/* Token Structure */}
          <section className="flex flex-col gap-4" id="tokens">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Token Structure</h2>
            <p className="text-muted-foreground leading-7">
              RootCX issues two JWT tokens on login and registration: a short-lived{" "}
              <strong className="text-foreground font-medium">access token</strong> for authenticating API
              requests, and a long-lived <strong className="text-foreground font-medium">refresh token</strong>{" "}
              for obtaining new access tokens without re-entering credentials.
            </p>

            <h3 className="text-lg font-semibold text-foreground mt-2">Access Token</h3>
            <p className="text-muted-foreground leading-7">
              Signed with HS256 using the runtime's JWT secret. Valid for{" "}
              <strong className="text-foreground font-medium">15 minutes</strong> from issuance. Contains the
              minimum claims needed to authenticate and authorize a request:
            </p>
            <CodeBlock
              language="json"
              code={`// Access token payload (decoded)
{
  "userId": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "username": "alice",
  "role": "user",
  "iat": 1736936200,
  "exp": 1736937100
}`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Refresh Token</h3>
            <p className="text-muted-foreground leading-7">
              Also HS256-signed. Valid for <strong className="text-foreground font-medium">30 days</strong> from
              issuance. Contains a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">sessionId</code> claim
              that references a row in the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">user_sessions</code> table.
              When the session is invalidated (via logout), refresh tokens for that session immediately become
              invalid even if they have not expired.
            </p>
            <CodeBlock
              language="json"
              code={`// Refresh token payload (decoded)
{
  "userId": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
  "sessionId": "session-uuid-here",
  "iat": 1736936200,
  "exp": 1739528200
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
              code={`curl https://your-runtime.example.com/api/v1/apps/my-app/collections/posts \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <Callout variant="warning" title="Store Tokens Securely">
              Never store access tokens or refresh tokens in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">localStorage</code> in
              browser environments — use <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">httpOnly</code> cookies
              or secure in-memory storage instead. Refresh tokens have a 30-day lifetime and must be protected accordingly.
            </Callout>
          </section>

          {/* Refresh */}
          <section className="flex flex-col gap-4" id="refresh">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Refresh Token</h2>
            <p className="text-muted-foreground leading-7">
              Exchanges a valid refresh token for a new short-lived access token. The refresh token itself
              is not rotated on refresh — it remains valid until logout or expiry. This simplifies token
              management for clients that may make concurrent refresh calls.
            </p>

            <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
              <div className="flex items-center gap-3">
                <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
                <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                  /api/v1/auth/refresh
                </code>
              </div>
            </div>

            <h3 className="text-lg font-semibold text-foreground mt-2">Request Body</h3>
            <PropertiesTable
              properties={[
                {
                  name: "refresh_token",
                  type: "string",
                  required: true,
                  description: "The refresh token returned from login or registration. Must be cryptographically valid, unexpired, and associated with an active session.",
                },
              ]}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
            <CodeBlock
              language="bash"
              code={`curl -X POST https://your-runtime.example.com/api/v1/auth/refresh \\
  -H "Content-Type: application/json" \\
  -d '{
    "refresh_token": "eyJhbGciOiJIUzI1NiJ9..."
  }'`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
            <CodeBlock
              language="json"
              code={`{
  "data": {
    "access_token": "eyJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiIwMTkyNmIzZS0xMjM0LTcwMDAtYWFhYS1iYmJiY2NjY2RkZGQiLCJ1c2VybmFtZSI6ImFsaWNlIiwicm9sZSI6InVzZXIiLCJpYXQiOjE3MzY5MzYyMDAsImV4cCI6MTczNjkzNzEwMH0.NEW_SIGNATURE"
  }
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
              until their 15-minute expiry — clients should discard them immediately upon logout.
            </p>

            <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
              <div className="flex items-center gap-3">
                <span className="text-xs font-mono font-bold text-blue-400 bg-blue-400/10 rounded px-2 py-0.5">POST</span>
                <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                  /api/v1/auth/logout
                </code>
              </div>
            </div>

            <p className="text-muted-foreground leading-7">
              Send the current access token in the Authorization header. The runtime extracts the session
              information from the token and marks the session as revoked in the database.
            </p>

            <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
            <CodeBlock
              language="bash"
              code={`curl -X POST https://your-runtime.example.com/api/v1/auth/logout \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
            <CodeBlock
              language="json"
              code={`{
  "data": {
    "message": "logged out successfully"
  }
}`}
            />
          </section>

          {/* Current User */}
          <section className="flex flex-col gap-4" id="current-user">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Current User</h2>
            <p className="text-muted-foreground leading-7">
              Returns the authenticated user's profile. Useful for populating UI after login, or for
              verifying that a stored access token is still valid. The response is derived from a live
              database lookup, not just the JWT payload, ensuring the latest role and profile data is returned.
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
              code={`curl https://your-runtime.example.com/api/v1/auth/me \\
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9..."`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Response — 200 OK</h3>
            <CodeBlock
              language="json"
              code={`{
  "data": {
    "id": "01926b3e-1234-7000-aaaa-bbbbccccdddd",
    "username": "alice",
    "email": "alice@example.com",
    "displayName": "Alice Smith",
    "role": "user",
    "created_at": "2025-01-15T10:30:00.000Z"
  }
}`}
            />
          </section>

          {/* Security */}
          <section className="flex flex-col gap-4" id="security">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Security</h2>

            <h3 className="text-lg font-semibold text-foreground mt-2">Password Hashing — Argon2id</h3>
            <p className="text-muted-foreground leading-7">
              All passwords are hashed using <strong className="text-foreground font-medium">Argon2id</strong>,
              the winner of the Password Hashing Competition and the current OWASP-recommended algorithm.
              RootCX uses the following parameters by default:
            </p>
            <CodeBlock
              language="text"
              code={`Memory:      65536 KiB (64 MiB)
Iterations:  3
Parallelism: 4
Hash length: 32 bytes
Salt length: 16 bytes (random per hash)`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">JWT Secret</h3>
            <p className="text-muted-foreground leading-7">
              The JWT secret used to sign all tokens is loaded in the following order of precedence:
            </p>
            <ol className="flex flex-col gap-1.5 text-muted-foreground text-sm leading-7 list-none pl-0">
              <li className="flex gap-2 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1">1.</span>
                <span>The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_JWT_SECRET</code> environment variable (preferred for containerized deployments)</span>
              </li>
              <li className="flex gap-2 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1">2.</span>
                <span>The file <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">config/jwt.key</code> in the runtime working directory</span>
              </li>
              <li className="flex gap-2 items-start">
                <span className="text-muted-foreground/50 font-mono text-xs mt-1">3.</span>
                <span>Auto-generated: a cryptographically random 32-byte key written to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">config/jwt.key</code> on first boot</span>
              </li>
            </ol>
            <Callout variant="warning" title="Persist Your JWT Secret">
              If the JWT secret changes (e.g. the container is replaced and the auto-generated key file is
              lost), all existing tokens are immediately invalidated and all users will be logged out. Always
              set <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_JWT_SECRET</code> explicitly
              in production, or mount <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">config/jwt.key</code> as
              a persistent volume.
            </Callout>

            <h3 className="text-lg font-semibold text-foreground mt-2">User Table Schema</h3>
            <CodeBlock
              language="sql"
              code={`CREATE TABLE IF NOT EXISTS users (
  id           TEXT PRIMARY KEY,
  username     TEXT NOT NULL UNIQUE,
  email        TEXT UNIQUE,
  display_name TEXT,
  password_hash TEXT NOT NULL,
  role         TEXT NOT NULL DEFAULT 'user',
  created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS users_username_lower_idx
  ON users (LOWER(username));`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">Session Table Schema</h3>
            <CodeBlock
              language="sql"
              code={`CREATE TABLE IF NOT EXISTS user_sessions (
  id          TEXT PRIMARY KEY,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  revoked     BOOLEAN NOT NULL DEFAULT FALSE,
  created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS user_sessions_user_id_idx
  ON user_sessions (user_id);`}
            />

            <h3 className="text-lg font-semibold text-foreground mt-2">RBAC Integration</h3>
            <p className="text-muted-foreground leading-7">
              When a user registers, the runtime assigns the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">user</code> role
              by default. To assign custom roles after registration — for example, elevating the first
              registered user to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">admin</code> —
              use the RBAC module's role assignment API or configure a post-registration Worker hook.
              The user's current role is always embedded in every newly-issued access token so role
              changes are reflected within 15 minutes without requiring a re-login.
            </p>
          </section>

        </div>

        <PageNav href="/modules/authentication" />
      </div>
    </DocsLayout>
  );
}
