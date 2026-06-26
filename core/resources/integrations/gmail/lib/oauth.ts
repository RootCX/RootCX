/// <reference path="../../rootcx-worker.d.ts" />
import { google, type Auth } from "googleapis";
import { LruMap } from "./lru";

export const GOOGLE_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";

export const BASE_SCOPES = [
  "email",
  "profile",
  "https://www.googleapis.com/auth/gmail.readonly",
  "https://www.googleapis.com/auth/gmail.send",
  "https://www.googleapis.com/auth/gmail.compose",
  "https://www.googleapis.com/auth/profile.emails.read",
];

export const MODIFY_SCOPE = "https://www.googleapis.com/auth/gmail.modify";

export interface Config {
  clientId?: string;
  clientSecret?: string;
  enableModifyScope?: boolean;
  pubsubTopicName?: string;
}

export interface UserCreds {
  refreshToken?: string;
}

export function scopesFor(c: Config): string {
  const scopes = [...BASE_SCOPES];
  if (c.enableModifyScope) scopes.push(MODIFY_SCOPE);
  return scopes.join(" ");
}

const CLIENT_CACHE = new LruMap<string, Auth.OAuth2Client>(64, 30 * 60_000);

export function oauth2ClientFor(config: Config, creds: UserCreds, userId: string, connectionId?: string | null): Auth.OAuth2Client {
  const cacheKey = connectionId ? `${userId}:${connectionId}` : userId;
  const cached = CLIENT_CACHE.get(cacheKey);
  if (cached) return cached;
  const client = new google.auth.OAuth2(config.clientId, config.clientSecret);
  client.setCredentials({ refresh_token: creds.refreshToken });
  CLIENT_CACHE.set(cacheKey, client);
  return client;
}

/** Build Google OAuth consent URL for a given tenant. */
export function buildAuthUrl(config: Config, callbackUrl: string, state: string): string {
  const url = new URL(GOOGLE_AUTH_URL);
  url.searchParams.set("client_id", config.clientId!);
  url.searchParams.set("redirect_uri", callbackUrl);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", scopesFor(config));
  url.searchParams.set("access_type", "offline");
  url.searchParams.set("prompt", "consent");
  if (state) url.searchParams.set("state", state);
  return url.toString();
}

/** Exchange authorization code for refresh_token + account email (from id_token). */
export async function exchangeCodeForRefreshToken(
  config: Config, code: string, redirectUri: string,
): Promise<{ refreshToken: string; email: string | null }> {
  const res = await fetch("https://oauth2.googleapis.com/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      code, client_id: config.clientId!, client_secret: config.clientSecret!,
      redirect_uri: redirectUri, grant_type: "authorization_code",
    }),
  });
  if (!res.ok) throw new Error(`token exchange failed: ${await res.text()}`);
  const data = await res.json();
  const email = parseIdTokenEmail(data.id_token);
  return { refreshToken: data.refresh_token, email };
}

export function parseIdTokenEmail(idToken: string | undefined): string | null {
  if (!idToken) return null;
  const parts = idToken.split(".");
  if (parts.length < 2) return null;
  const payload = JSON.parse(Buffer.from(parts[1], "base64url").toString());
  return payload.email?.toLowerCase() ?? null;
}

export function evictClient(userId: string, connectionId?: string | null): void {
  const cacheKey = connectionId ? `${userId}:${connectionId}` : userId;
  CLIENT_CACHE.delete(cacheKey);
}

