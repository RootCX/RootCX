import { useState, type ReactNode } from "react";
import { useAuth } from "../hooks/useAuth";
import type { AuthUser } from "../client";

export interface AuthFormSlotProps {
  mode: "login" | "register";
  setMode: (mode: "login" | "register") => void;
  error: string | null;
  submitting: boolean;
  onSubmit: (e: React.FormEvent<HTMLFormElement>) => void;
  appTitle: string;
}

export interface AuthGateProps {
  /** Default: "Sign in" */
  appTitle?: string;
  renderLoading?: () => ReactNode;
  renderForm?: (props: AuthFormSlotProps) => ReactNode;
  children: (auth: { user: AuthUser; logout: () => Promise<void> }) => ReactNode;
}

function friendlyAuthError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const l = raw.toLowerCase();
  if (l.includes("already taken")) return "This username is already taken.";
  if (l.includes("invalid credentials")) return "Wrong username or password.";
  if (l.includes("session revoked") || l.includes("expired"))
    return "Your session has expired. Please sign in again.";
  if (l.includes("fetch") || l.includes("network") || l.includes("failed to fetch"))
    return "Unable to reach the server. Please check your connection.";
  // Surface server validation messages (e.g. password policy)
  try {
    const json = raw.match(/\{[\s\S]*\}$/)?.[0];
    if (json) { const parsed = JSON.parse(json); if (typeof parsed.error === "string") return parsed.error; }
  } catch {}
  return raw || "Something went wrong. Please try again.";
}

const inputCls =
  "flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";

function DefaultAuthForm({ mode, setMode, error, submitting, onSubmit, appTitle }: AuthFormSlotProps) {
  return (
    <div className="flex min-h-screen items-center justify-center bg-background px-4">
      <div className="w-full max-w-sm rounded-lg border bg-card p-6 shadow-sm">
        <div className="mb-6">
          <h2 className="text-2xl font-semibold tracking-tight">{appTitle}</h2>
          <p className="text-sm text-muted-foreground">
            {mode === "login" ? "Sign in to your account" : "Create a new account"}
          </p>
        </div>
        <form className="space-y-4" onSubmit={onSubmit}>
          {error && (
            <div className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</div>
          )}
          <div className="space-y-2">
            <label htmlFor="username" className="text-sm font-medium leading-none">Username</label>
            <input id="username" name="username" placeholder="Username" autoComplete="username" required disabled={submitting} className={inputCls} />
          </div>
          <div className="space-y-2">
            <label htmlFor="password" className="text-sm font-medium leading-none">Password</label>
            <input id="password" name="password" type="password" placeholder="Password" autoComplete={mode === "login" ? "current-password" : "new-password"} minLength={8} required disabled={submitting} className={inputCls} />
            {mode === "register" && <p className="text-xs text-muted-foreground">Must be at least 8 characters.</p>}
          </div>
          {mode === "register" && (
            <div className="space-y-2">
              <label htmlFor="confirmPassword" className="text-sm font-medium leading-none">Confirm password</label>
              <input id="confirmPassword" name="confirmPassword" type="password" placeholder="Confirm password" autoComplete="new-password" minLength={8} required disabled={submitting} className={inputCls} />
            </div>
          )}
          <button type="submit" disabled={submitting} className="inline-flex h-10 w-full items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:pointer-events-none disabled:opacity-50">
            {submitting
              ? mode === "login" ? "Signing in…" : "Creating account…"
              : mode === "login" ? "Sign in" : "Create account"}
          </button>
          <p className="text-center text-sm text-muted-foreground">
            {mode === "login" ? "No account? " : "Already have one? "}
            <button type="button" className="text-primary underline-offset-4 hover:underline" disabled={submitting} onClick={() => setMode(mode === "login" ? "register" : "login")}>
              {mode === "login" ? "Register" : "Sign in"}
            </button>
          </p>
        </form>
      </div>
    </div>
  );
}

export function AuthGate({ appTitle = "Sign in", renderLoading, renderForm, children }: AuthGateProps) {
  const { user, loading, login, register, logout } = useAuth();
  const [mode, setMode] = useState<"login" | "register">("login");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  if (loading) {
    return renderLoading ? <>{renderLoading()}</> : (
      <div className="flex min-h-screen items-center justify-center">
        <p className="text-muted-foreground">Loading…</p>
      </div>
    );
  }

  if (!user) {
    const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
      e.preventDefault();
      setError(null);
      const fd = new FormData(e.currentTarget);
      const username = (fd.get("username") as string).trim();
      const password = fd.get("password") as string;

      if (mode === "register") {
        const confirm = fd.get("confirmPassword") as string;
        if (password !== confirm) { setError("Passwords do not match."); return; }
      }

      setSubmitting(true);
      try {
        await (mode === "register" ? register({ username, password }) : login(username, password));
      } catch (err) {
        setError(friendlyAuthError(err));
      } finally {
        setSubmitting(false);
      }
    };

    const formProps: AuthFormSlotProps = {
      mode,
      setMode: (m) => { setMode(m); setError(null); },
      error, submitting, onSubmit: handleSubmit, appTitle,
    };

    return <>{renderForm ? renderForm(formProps) : <DefaultAuthForm {...formProps} />}</>;
  }

  return <>{children({ user, logout })}</>;
}
