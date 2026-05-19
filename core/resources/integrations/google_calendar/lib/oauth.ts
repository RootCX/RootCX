/// <reference path="../../rootcx-worker.d.ts" />
import { google, type Auth } from "googleapis";
import { LruMap } from "./lru";

export const GOOGLE_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";

export const SCOPES = [
  "email",
  "profile",
  "https://www.googleapis.com/auth/calendar",
];

export interface Config {
  clientId: string;
  clientSecret: string;
}

export interface UserCreds {
  refreshToken?: string;
}

const CLIENT_CACHE = new LruMap<string, Auth.OAuth2Client>(64, 30 * 60_000);

export function oauth2ClientFor(config: Config, creds: UserCreds, userId: string): Auth.OAuth2Client {
  const cached = CLIENT_CACHE.get(userId);
  if (cached) return cached;
  const client = new google.auth.OAuth2(config.clientId, config.clientSecret);
  client.setCredentials({ refresh_token: creds.refreshToken });
  CLIENT_CACHE.set(userId, client);
  return client;
}

export function authUrl(config: Config, callbackUrl: string, state: string): string {
  const url = new URL(GOOGLE_AUTH_URL);
  url.searchParams.set("client_id", config.clientId);
  url.searchParams.set("redirect_uri", callbackUrl);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", SCOPES.join(" "));
  url.searchParams.set("access_type", "offline");
  url.searchParams.set("prompt", "consent");
  if (state) url.searchParams.set("state", state);
  return url.toString();
}

export async function exchangeCodeForRefreshToken(
  config: Config, code: string, redirectUri: string,
): Promise<string> {
  const res = await fetch("https://oauth2.googleapis.com/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      code, client_id: config.clientId, client_secret: config.clientSecret,
      redirect_uri: redirectUri, grant_type: "authorization_code",
    }),
  });
  if (!res.ok) throw new Error(`token exchange failed: ${await res.text()}`);
  const data = await res.json();
  return data.refresh_token;
}

export function evictClient(userId: string): void {
  CLIENT_CACHE.delete(userId);
}
