import { useCallback, useEffect, useState } from "react";
import type { UseAuthResult } from "./useAuth";
import { useRuntimeClient } from "../components/RuntimeProvider";

type Attempt = "pending" | "signed-out" | "unavailable" | null;

interface OidcRedirect {
  redirecting: boolean;
  error: unknown;
  start: (providerId: string) => Promise<void>;
  signOut: () => Promise<void>;
}

export function useOidcRedirect(auth: UseAuthResult, enabled: boolean): OidcRedirect {
  const key = `rootcx_oidc_redirect:${useRuntimeClient().getBaseUrl()}`;
  const [attempt, setAttempt] = useState<Attempt>(() => {
    try { return sessionStorage.getItem(key) as Attempt; }
    catch { return "unavailable"; }
  });
  const [redirecting, setRedirecting] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const { user, loading, authMode, authError, oidcLogin, logout } = auth;
  const providerId = authMode?.providers.length === 1 ? authMode.providers[0].id : null;
  const shouldRedirect = !loading && !user && enabled && providerId !== null && !attempt && !authError;

  const remember = useCallback((value: Attempt) => {
    setAttempt(value);
    try {
      if (value) sessionStorage.setItem(key, value);
      else sessionStorage.removeItem(key);
    } catch { /* Manual login and logout remain usable without session storage. */ }
  }, [key]);

  const start = useCallback(async (providerId: string) => {
    remember("pending");
    setError(null);
    setRedirecting(true);
    try {
      await oidcLogin(providerId);
    } catch (err) {
      setError(err);
      setRedirecting(false);
    }
  }, [oidcLogin, remember]);

  useEffect(() => {
    if (!loading && user) {
      remember(null);
      setRedirecting(false);
      setError(null);
    }
  }, [loading, user, remember]);

  useEffect(() => {
    if (!shouldRedirect || providerId === null) return;
    // The marker survives navigation and prevents retry loops after a failed callback.
    try {
      const existing = sessionStorage.getItem(key) as Attempt;
      if (existing) {
        setAttempt(existing);
        return;
      }
      sessionStorage.setItem(key, "pending");
    } catch {
      setAttempt("unavailable");
      return;
    }
    void start(providerId);
  }, [shouldRedirect, key, providerId, start]);

  useEffect(() => {
    function onPageShow(event: PageTransitionEvent): void {
      if (event.persisted) setRedirecting(false);
    }
    window.addEventListener("pageshow", onPageShow);
    return () => window.removeEventListener("pageshow", onPageShow);
  }, []);

  const signOut = useCallback(async () => {
    remember("signed-out");
    try {
      await logout();
    } catch (err) {
      remember(null);
      throw err;
    }
  }, [logout, remember]);

  let signInError = error || authError || null;
  if (!signInError && !user && !redirecting && attempt === "pending") {
    signInError = "Sign-in did not complete. Please try again.";
  }

  return {
    redirecting: redirecting || shouldRedirect,
    error: signInError,
    start,
    signOut,
  };
}
