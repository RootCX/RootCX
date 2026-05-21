import { useCallback, useEffect, useState } from "react";
import { type AuthUser, type AuthMode, type RegisterInput } from "../client";
import { useRuntimeClient, REFRESH_KEY } from "../components/RuntimeProvider";

export interface UseAuthResult {
  user: AuthUser | null;
  loading: boolean;
  isAuthenticated: boolean;
  authMode: AuthMode | null;
  login: (email: string, password: string) => Promise<void>;
  register: (data: RegisterInput) => Promise<void>;
  logout: () => Promise<void>;
  oidcLogin: (providerId: string) => Promise<void>;
}

function consumeOidcTokensFromUrl(): { accessToken: string; refreshToken: string } | null {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const accessToken = params.get("access_token");
  const refreshToken = params.get("refresh_token");
  if (!accessToken || !refreshToken) return null;

  params.delete("access_token");
  params.delete("refresh_token");
  params.delete("expires_in");
  const clean = params.toString();
  const url = window.location.pathname + (clean ? `?${clean}` : "");
  window.history.replaceState({}, "", url);

  return { accessToken, refreshToken };
}

export function useAuth(): UseAuthResult {
  const client = useRuntimeClient();
  const [user, setUser] = useState<AuthUser | null>(null);
  const [loading, setLoading] = useState(true);
  const [authMode, setAuthMode] = useState<AuthMode | null>(null);

  useEffect(() => {
    // Check for OIDC callback tokens in URL (browser redirect flow)
    const oidcTokens = consumeOidcTokensFromUrl();
    if (oidcTokens) {
      client.setTokens(oidcTokens.accessToken, oidcTokens.refreshToken);
      localStorage.setItem(REFRESH_KEY, oidcTokens.refreshToken);
    }

    const init = async () => {
      const [mode] = await Promise.all([
        client.authMode().catch(() => null),
        client.me().then(setUser).catch(() => {
          setUser(null);
          localStorage.removeItem(REFRESH_KEY);
        }),
      ]);
      setAuthMode(mode);
      setLoading(false);
    };
    init();
  }, [client]);

  const persistTokens = useCallback(() => {
    const refresh = client.getRefreshToken();
    if (refresh) localStorage.setItem(REFRESH_KEY, refresh);
    else localStorage.removeItem(REFRESH_KEY);
  }, [client]);

  const login = useCallback(
    async (email: string, password: string) => {
      const res = await client.login(email, password);
      persistTokens();
      setUser(res.user);
    },
    [client, persistTokens],
  );

  const register = useCallback(
    async (data: RegisterInput) => {
      await client.register(data);
      const res = await client.login(data.email, data.password);
      persistTokens();
      setUser(res.user);
    },
    [client, persistTokens],
  );

  const logout = useCallback(async () => {
    await client.logout();
    persistTokens();
    setUser(null);
  }, [client, persistTokens]);

  const oidcLogin = useCallback(
    async (providerId: string) => {
      await client.oidcLogin(providerId);
      // Tauri path: tokens set on client, fetch user
      persistTokens();
      const me = await client.me();
      setUser(me);
    },
    [client, persistTokens],
  );

  return {
    user,
    loading,
    isAuthenticated: user !== null,
    authMode,
    login,
    register,
    logout,
    oidcLogin,
  };
}
