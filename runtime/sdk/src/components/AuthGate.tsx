import type { ReactNode } from "react";
import { useAuth } from "../hooks/useAuth";
import { useOidcRedirect } from "../hooks/useOidcRedirect";
import type { AuthUser, OidcProvider } from "../client";

export interface AuthFormSlotProps {
  error: string | null;
  submitting: boolean;
  appTitle: string;
  providers: OidcProvider[];
  onOidcLogin: (providerId: string) => void;
}

export interface AuthGateProps {
  /** Default: "Sign in" */
  appTitle?: string;
  /** Automatically use the sole configured SSO provider. */
  autoOidcLogin?: boolean;
  renderLoading?: () => ReactNode;
  /** Compose the form with the application's UI package. The SDK owns authentication only. */
  renderForm: (props: AuthFormSlotProps) => ReactNode;
  children: (auth: { user: AuthUser; logout: () => Promise<void> }) => ReactNode;
}

function friendlyAuthError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const lower = raw.toLowerCase();
  if (lower.includes("session revoked") || lower.includes("expired"))
    return "Your session has expired. Please sign in again.";
  if (lower.includes("fetch") || lower.includes("network"))
    return "Unable to reach the server. Please check your connection.";
  try {
    const json = raw.match(/\{[\s\S]*\}$/)?.[0];
    if (json) {
      const parsed = JSON.parse(json);
      if (typeof parsed.error === "string") return parsed.error;
    }
  } catch {}
  return raw || "Something went wrong. Please try again.";
}

export function AuthGate({ appTitle = "Sign in", autoOidcLogin = true, renderLoading, renderForm, children }: AuthGateProps): ReactNode {
  const auth = useAuth();
  const { user, loading, authMode } = auth;
  const sso = useOidcRedirect(auth, autoOidcLogin);
  if (loading || sso.redirecting) {
    return renderLoading ? <>{renderLoading()}</> : (
      <p role="status">Loading…</p>
    );
  }

  if (user) return <>{children({ user, logout: sso.signOut })}</>;

  return <>{renderForm({
    error: sso.error ? friendlyAuthError(sso.error) : null,
    submitting: sso.redirecting,
    appTitle,
    providers: authMode?.providers ?? [],
    onOidcLogin: providerId => { void sso.start(providerId); },
  })}</>;
}
