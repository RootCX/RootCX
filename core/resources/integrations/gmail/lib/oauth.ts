/// <reference path="../../rootcx-worker.d.ts" />
import { google, type Auth } from "googleapis";

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
  proxyToken?: string;
  baseUrl?: string;
  enableModifyScope?: boolean;
  pubsubTopicName?: string;
}

export interface UserCreds {
  refreshToken?: string;
  managed?: boolean;
}

export const isManaged = (c: Config): boolean => !!(c.proxyToken && c.baseUrl);

export function scopesFor(c: Config): string {
  const scopes = [...BASE_SCOPES];
  if (c.enableModifyScope) scopes.push(MODIFY_SCOPE);
  return scopes.join(" ");
}

import { LruMap } from "./lru";

const CLIENT_CACHE = new LruMap<string, Auth.OAuth2Client>(64, 30 * 60_000);

export function oauth2ClientFor(config: Config, creds: UserCreds, userId: string): Auth.OAuth2Client {
  const cached = CLIENT_CACHE.get(userId);
  if (cached) return cached;
  const client = new google.auth.OAuth2(config.clientId, config.clientSecret);
  client.setCredentials({ refresh_token: creds.refreshToken });
  CLIENT_CACHE.set(userId, client);
  return client;
}

/** Managed-mode token fetch. The proxy returns a short-lived access token. */
export async function managedAccessToken(config: Config, userId: string): Promise<string> {
  const res = await fetch(`${config.baseUrl}/token`, {
    method: "POST",
    headers: {
      "Authorization": `Bearer ${config.proxyToken}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ userId }),
  });
  if (!res.ok) throw new Error(`managed token fetch failed: ${await res.text()}`);
  const data = await res.json();
  return data.accessToken;
}

/** Self-hosted OAuth: redirect to Google consent. */
export function selfHostedAuthUrl(config: Config, callbackUrl: string, state: string): string {
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

/** Managed-mode: redirect to proxy. */
export function managedAuthUrl(config: Config, callbackUrl: string, state: string, userId: string): string {
  const [tenantRef, ...hmacParts] = config.proxyToken!.split(":");
  const url = new URL(`${config.baseUrl}/auth/start`);
  url.searchParams.set("callback_url", callbackUrl);
  url.searchParams.set("state", state);
  url.searchParams.set("tenant_ref", tenantRef);
  url.searchParams.set("user_id", userId);
  url.searchParams.set("hmac", hmacParts.join(":"));
  return url.toString();
}

/** Exchange authorization code for refresh_token (self-hosted only). */
export async function exchangeCodeForRefreshToken(
  config: Config, code: string, redirectUri: string,
): Promise<string> {
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
  return data.refresh_token;
}

export function evictClient(userId: string): void {
  CLIENT_CACHE.delete(userId);
}

