import { useCallback, useEffect, useRef, useState } from "react";
import {
  type AuthUser,
  type RegisterInput,
  RuntimeClient,
} from "../client";

export interface UseAuthResult {
  user: AuthUser | null;
  loading: boolean;
  isAuthenticated: boolean;
  login: (username: string, password: string) => Promise<void>;
  register: (data: RegisterInput) => Promise<void>;
  logout: () => Promise<void>;
}

const TOKEN_KEY = "rootcx_access_token";
const REFRESH_KEY = "rootcx_refresh_token";

export function useAuth(opts?: { baseUrl?: string }): UseAuthResult {
  const clientRef = useRef(new RuntimeClient({ baseUrl: opts?.baseUrl }));
  const [user, setUser] = useState<AuthUser | null>(null);
  const [loading, setLoading] = useState(true);

  // Restore tokens from localStorage on mount
  useEffect(() => {
    const access = localStorage.getItem(TOKEN_KEY);
    const refresh = localStorage.getItem(REFRESH_KEY);
    if (access || refresh) {
      clientRef.current.setTokens(access, refresh);
    }

    // Validate existing token
    clientRef.current
      .me()
      .then(setUser)
      .catch(() => {
        setUser(null);
        localStorage.removeItem(TOKEN_KEY);
        localStorage.removeItem(REFRESH_KEY);
      })
      .finally(() => setLoading(false));
  }, []);

  const persistTokens = useCallback(() => {
    const access = clientRef.current.getAccessToken();
    const refresh = clientRef.current.getRefreshToken();
    if (access) localStorage.setItem(TOKEN_KEY, access);
    else localStorage.removeItem(TOKEN_KEY);
    if (refresh) localStorage.setItem(REFRESH_KEY, refresh);
    else localStorage.removeItem(REFRESH_KEY);
  }, []);

  const login = useCallback(
    async (username: string, password: string) => {
      const res = await clientRef.current.login(username, password);
      persistTokens();
      setUser(res.user);
    },
    [persistTokens],
  );

  const register = useCallback(
    async (data: RegisterInput) => {
      await clientRef.current.register(data);
    },
    [],
  );

  const logout = useCallback(async () => {
    await clientRef.current.logout();
    persistTokens();
    setUser(null);
  }, [persistTokens]);

  return {
    user,
    loading,
    isAuthenticated: user !== null,
    login,
    register,
    logout,
  };
}
