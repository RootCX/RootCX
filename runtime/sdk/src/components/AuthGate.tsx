import { useState, type ReactNode } from "react";
import { useAuth } from "../hooks/useAuth";
import type { AuthUser, OidcProvider } from "../client";

export interface AuthFormSlotProps {
  mode: "login" | "register";
  setMode: (mode: "login" | "register") => void;
  error: string | null;
  submitting: boolean;
  onSubmit: (e: React.FormEvent<HTMLFormElement>) => void;
  appTitle: string;
  providers: OidcProvider[];
  onOidcLogin: (providerId: string) => void;
  passwordLoginEnabled: boolean;
}

export interface AuthGateProps {
  /** Default: "Sign in" */
  appTitle?: string;
  renderLoading?: () => ReactNode;
  /** Compose the form with the application's UI package. The SDK owns authentication only. */
  renderForm: (props: AuthFormSlotProps) => ReactNode;
  children: (auth: { user: AuthUser; logout: () => Promise<void> }) => ReactNode;
}

function friendlyAuthError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const l = raw.toLowerCase();
  if (l.includes("already taken")) return "This email is already taken.";
  if (l.includes("invalid credentials")) return "Wrong email or password.";
  if (l.includes("session revoked") || l.includes("expired"))
    return "Your session has expired. Please sign in again.";
  if (l.includes("fetch") || l.includes("network") || l.includes("failed to fetch"))
    return "Unable to reach the server. Please check your connection.";
  try {
    const json = raw.match(/\{[\s\S]*\}$/)?.[0];
    if (json) { const parsed = JSON.parse(json); if (typeof parsed.error === "string") return parsed.error; }
  } catch {}
  return raw || "Something went wrong. Please try again.";
}

export function AuthGate({ appTitle = "Sign in", renderLoading, renderForm, children }: AuthGateProps) {
  const { user, loading, login, register, logout, oidcLogin, authMode } = useAuth();
  const [mode, setMode] = useState<"login" | "register">("login");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  if (loading) {
    return renderLoading ? <>{renderLoading()}</> : (
      <p role="status">Loading\u2026</p>
    );
  }

  if (!user) {
    const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
      e.preventDefault();
      setError(null);
      const fd = new FormData(e.currentTarget);
      const email = (fd.get("email") as string).trim();
      const password = fd.get("password") as string;

      if (mode === "register") {
        const confirm = fd.get("confirmPassword") as string;
        if (password !== confirm) { setError("Passwords do not match."); return; }
      }

      setSubmitting(true);
      try {
        await (mode === "register" ? register({ email, password }) : login(email, password));
      } catch (err) {
        setError(friendlyAuthError(err));
      } finally {
        setSubmitting(false);
      }
    };

    const handleOidcLogin = (providerId: string) => {
      setError(null);
      setSubmitting(true);
      oidcLogin(providerId)
        .catch((err) => setError(friendlyAuthError(err)))
        .finally(() => setSubmitting(false));
    };

    const providers = authMode?.providers ?? [];
    const passwordLoginEnabled = authMode?.passwordLoginEnabled ?? true;

    const formProps: AuthFormSlotProps = {
      mode,
      setMode: (m) => { setMode(m); setError(null); },
      error, submitting, onSubmit: handleSubmit, appTitle,
      providers, onOidcLogin: handleOidcLogin, passwordLoginEnabled,
    };

    return <>{renderForm(formProps)}</>;
  }

  return <>{children({ user, logout })}</>;
}
