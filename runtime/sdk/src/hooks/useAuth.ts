import { useCallback, useEffect, useRef, useState } from "react";
import type { AuthMode, AuthUser, RuntimeClient } from "../client";
import { useRuntimeClient, REFRESH_KEY } from "../components/RuntimeProvider";

export interface UseAuthResult {
  user: AuthUser | null;
  loading: boolean;
  isAuthenticated: boolean;
  authMode: AuthMode | null;
  authError: string | null;
  logout: () => Promise<void>;
  oidcLogin: (providerId: string) => Promise<void>;
  magicLinkConsume: (token: string) => Promise<void>;
}

interface AuthInitialization {
  mode: AuthMode | null;
  user: AuthUser | null;
  error: string | null;
}

function replaceUrlQuery(params: URLSearchParams, hash = window.location.hash): void {
  const query = params.toString();
  window.history.replaceState({}, "", window.location.pathname + (query ? `?${query}` : "") + hash);
}

function consumeCallbackTokensFromUrl(): { accessToken: string; refreshToken: string } | null {
  if (typeof window === "undefined") return null;

  const query = new URLSearchParams(window.location.search);
  const fragment = new URLSearchParams(window.location.hash.slice(1));
  let accessToken = query.get("access_token");
  let refreshToken = query.get("refresh_token");

  if (!accessToken || !refreshToken) {
    accessToken = fragment.get("access_token");
    refreshToken = fragment.get("refresh_token");
  }

  if (!accessToken || !refreshToken) return null;

  // Legacy callbacks can carry credentials in both locations.
  query.delete("access_token");
  query.delete("refresh_token");
  query.delete("expires_in");
  const hash = fragment.has("access_token") || fragment.has("refresh_token") ? "" : window.location.hash;
  replaceUrlQuery(query, hash);
  return { accessToken, refreshToken };
}

function getAuthNonce(): string | null {
  if (typeof window === "undefined") return null;
  return new URLSearchParams(window.location.search).get("auth_nonce");
}

function clearAuthNonce(): void {
  const params = new URLSearchParams(window.location.search);
  params.delete("auth_nonce");
  replaceUrlQuery(params);
}

async function initializeAuth(client: RuntimeClient): Promise<AuthInitialization> {
  let error: string | null = null;
  const callbackTokens = consumeCallbackTokensFromUrl();
  if (callbackTokens) {
    client.setTokens(callbackTokens.accessToken, callbackTokens.refreshToken);
    localStorage.setItem(REFRESH_KEY, callbackTokens.refreshToken);
  }

  const authNonce = callbackTokens ? null : getAuthNonce();
  if (authNonce) {
    try {
      const tokens = await client.exchangeNonce(authNonce);
      client.setTokens(tokens.accessToken, tokens.refreshToken);
      localStorage.setItem(REFRESH_KEY, tokens.refreshToken);
    } catch {
      error = "Sign-in did not complete. Please try again.";
    } finally {
      clearAuthNonce();
    }
  }

  const [mode, user] = await Promise.all([
    client.authMode().catch(() => null),
    client.me().catch(() => {
      localStorage.removeItem(REFRESH_KEY);
      return null;
    }),
  ]);
  return { mode, user, error };
}

export function useAuth(): UseAuthResult {
  const client = useRuntimeClient();
  const [user, setUser] = useState<AuthUser | null>(null);
  const [loading, setLoading] = useState(true);
  const [authMode, setAuthMode] = useState<AuthMode | null>(null);
  const [authError, setAuthError] = useState<string | null>(null);
  const initialization = useRef<{
    client: RuntimeClient;
    promise: Promise<AuthInitialization>;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;

    // StrictMode replays effects; a single-use callback nonce must only be exchanged once.
    if (initialization.current?.client !== client) {
      initialization.current = { client, promise: initializeAuth(client) };
    }
    initialization.current.promise.then(({ mode, user: currentUser, error }) => {
      if (cancelled) return;
      setAuthMode(mode);
      setUser(currentUser);
      setAuthError(currentUser ? null : error);
      setLoading(false);
    });

    return () => { cancelled = true; };
  }, [client]);

  const persistTokens = useCallback(() => {
    const refresh = client.getRefreshToken();
    if (refresh) localStorage.setItem(REFRESH_KEY, refresh);
    else localStorage.removeItem(REFRESH_KEY);
  }, [client]);

  const logout = useCallback(async () => {
    await client.logout();
    persistTokens();
    setUser(null);
  }, [client, persistTokens]);

  const oidcLogin = useCallback(
    async (providerId: string) => {
      await client.oidcLogin(providerId);
    },
    [client],
  );

  const magicLinkConsume = useCallback(
    async (token: string) => {
      const res = await client.magicLinkConsume(token);
      persistTokens();
      setAuthError(null);
      setUser(res.user);
    },
    [client, persistTokens],
  );

  return {
    user,
    loading,
    isAuthenticated: user !== null,
    authMode,
    authError,
    logout,
    oidcLogin,
    magicLinkConsume,
  };
}
