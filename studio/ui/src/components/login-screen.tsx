import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { login, register } from "@/core/auth";

const ERROR_MAP: Record<string, string> = {
  "invalid credentials": "Incorrect username or password",
  "password login not available": "Password login is not available for this account",
  "username required, password min 8 chars": "Username is required and password must be at least 8 characters",
  "session revoked or expired": "Your session has expired, please sign in again",
};

function friendlyError(raw: string): string {
  let msg = raw;
  try {
    const parsed = JSON.parse(raw);
    if (typeof parsed?.error === "string") msg = parsed.error;
    else if (typeof parsed?.message === "string") msg = parsed.message;
  } catch { /* not JSON */ }
  return ERROR_MAP[msg] ?? msg;
}

const INPUT = "rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground outline-none focus:border-primary";

export function LoginScreen() {
  const [mode, setMode] = useState<"login" | "register">("login");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [email, setEmail] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      if (mode === "login") await login(username, password);
      else await register({ username, password, email: email || undefined, displayName: displayName || undefined });
    } catch (err) {
      setError(friendlyError(err instanceof Error ? err.message : String(err)));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="relative flex h-screen w-screen items-center justify-center overflow-hidden bg-background">
      <Logo className="pointer-events-none absolute h-[65%] max-h-[440px] text-white/[0.025]" />

      <form onSubmit={handleSubmit} className="z-10 flex w-80 flex-col gap-3 rounded-lg border border-border bg-sidebar p-6">
        <h1 className="mb-1 text-center text-lg font-semibold text-foreground">
          {mode === "login" ? "Sign in" : "Create account"}
        </h1>

        <label className="flex flex-col gap-1 text-xs text-muted-foreground">
          Username
          <input type="text" required autoFocus value={username} onChange={(e) => setUsername(e.target.value)} className={INPUT} />
        </label>

        <label className="flex flex-col gap-1 text-xs text-muted-foreground">
          Password
          <input type="password" required minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} className={INPUT} />
        </label>

        {mode === "register" && (
          <>
            <label className="flex flex-col gap-1 text-xs text-muted-foreground">
              Email
              <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} className={INPUT} />
            </label>
            <label className="flex flex-col gap-1 text-xs text-muted-foreground">
              Display name
              <input type="text" value={displayName} onChange={(e) => setDisplayName(e.target.value)} className={INPUT} />
            </label>
          </>
        )}

        {error && <div className="rounded-md border border-red-900/50 bg-red-950/30 px-3 py-2 text-xs text-red-400">{error}</div>}

        <Button type="submit" disabled={submitting} className="mt-1 cursor-pointer">
          {submitting ? "..." : mode === "login" ? "Sign in" : "Create account"}
        </Button>

        <button type="button" onClick={() => { setMode(mode === "login" ? "register" : "login"); setError(null); }}
          className="cursor-pointer text-xs text-muted-foreground hover:text-foreground">
          {mode === "login" ? "Don't have an account? Register" : "Already have an account? Sign in"}
        </button>
      </form>
    </div>
  );
}
