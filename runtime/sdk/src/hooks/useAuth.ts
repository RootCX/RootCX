import { useCallback, useEffect, useState } from "react";
import { type AuthUser, type RegisterInput } from "../client";
import { useRuntimeClient, TOKEN_KEY, REFRESH_KEY } from "../components/RuntimeProvider";

export interface UseAuthResult {
  user: AuthUser | null;
  loading: boolean;
  isAuthenticated: boolean;
  login: (username: string, password: string) => Promise<void>;
  register: (data: RegisterInput) => Promise<void>;
  logout: () => Promise<void>;
}

export function useAuth(): UseAuthResult {
  const client = useRuntimeClient();
  const [user, setUser] = useState<AuthUser | null>(null);
  const [loading, setLoading] = useState(true);

  // Validate existing session on mount.
  // Tokens are already restored by RuntimeProvider, so we just call me().
  useEffect(() => {
    client
      .me()
      .then(setUser)
      .catch(() => {
        setUser(null);
        localStorage.removeItem(TOKEN_KEY);
        localStorage.removeItem(REFRESH_KEY);
      })
      .finally(() => setLoading(false));
  }, [client]);

  const persistTokens = useCallback(() => {
    const access = client.getAccessToken();
    const refresh = client.getRefreshToken();
    if (access) localStorage.setItem(TOKEN_KEY, access);
    else localStorage.removeItem(TOKEN_KEY);
    if (refresh) localStorage.setItem(REFRESH_KEY, refresh);
    else localStorage.removeItem(REFRESH_KEY);
  }, [client]);

  const login = useCallback(
    async (username: string, password: string) => {
      const res = await client.login(username, password);
      persistTokens();
      setUser(res.user);
    },
    [client, persistTokens],
  );

  const register = useCallback(
    async (data: RegisterInput) => {
      await client.register(data);
    },
    [client],
  );

  const logout = useCallback(async () => {
    await client.logout();
    persistTokens();
    setUser(null);
  }, [client, persistTokens]);

  return {
    user,
    loading,
    isAuthenticated: user !== null,
    login,
    register,
    logout,
  };
}
