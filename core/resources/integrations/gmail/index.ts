/// <reference path="../rootcx-worker.d.ts" />
import { convert } from "html-to-text";
import { google } from "googleapis";
import { batchFetchImplementation } from "@jrmdayn/googleapis-batcher";

const GMAIL_API = "https://www.googleapis.com/gmail/v1/users/me";
const GOOGLE_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
const SCOPES = "https://www.googleapis.com/auth/gmail.send https://www.googleapis.com/auth/gmail.readonly";

interface Config {
  clientId?: string;
  clientSecret?: string;
  proxyToken?: string;
  baseUrl?: string;
}
interface UserCreds { refreshToken?: string; accessToken?: string; expiresAt?: number; managed?: boolean }

const isManaged = (c: Config) => !!(c.proxyToken && c.baseUrl);

async function authStart(params: any) {
  const { config, callbackUrl, state, userId } = params;

  if (isManaged(config)) {
    const [tenantRef, ...hmacParts] = config.proxyToken!.split(":");
    const url = new URL(`${config.baseUrl}/auth/start`);
    url.searchParams.set("callback_url", callbackUrl);
    url.searchParams.set("state", state);
    url.searchParams.set("tenant_ref", tenantRef);
    url.searchParams.set("user_id", userId);
    url.searchParams.set("hmac", hmacParts.join(":"));
    return { type: "redirect", url: url.toString() };
  }

  const url = new URL(GOOGLE_AUTH_URL);
  url.searchParams.set("client_id", config.clientId!);
  url.searchParams.set("redirect_uri", callbackUrl);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", SCOPES);
  url.searchParams.set("access_type", "offline");
  url.searchParams.set("prompt", "consent");
  if (state) url.searchParams.set("state", state);
  return { type: "redirect", url: url.toString() };
}

async function authCallback(params: any) {
  const { config, query } = params;

  if (isManaged(config) || query.code === "MANAGED_OK") {
    return { credentials: { managed: true } };
  }

  const res = await fetch(GOOGLE_TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      code: query.code,
      client_id: config.clientId!,
      client_secret: config.clientSecret!,
      redirect_uri: query.redirect_uri ?? params.callbackUrl ?? "",
      grant_type: "authorization_code",
    }),
  });
  if (!res.ok) throw new Error(`token exchange failed: ${await res.text()}`);
  const data = await res.json();
  return {
    credentials: {
      refreshToken: data.refresh_token,
      accessToken: data.access_token,
      expiresAt: Date.now() + data.expires_in * 1000,
    },
  };
}

async function getAccessToken(config: Config, creds: UserCreds, userId?: string): Promise<string> {
  if (isManaged(config)) {
    if (!userId) throw new Error("userId required for managed token fetch");
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

  if (creds.accessToken && creds.expiresAt && Date.now() < creds.expiresAt - 30_000) {
    return creds.accessToken;
  }
  const res = await fetch(GOOGLE_TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: config.clientId!,
      client_secret: config.clientSecret!,
      refresh_token: creds.refreshToken!,
      grant_type: "refresh_token",
    }),
  });
  if (!res.ok) throw new Error(`token refresh failed: ${await res.text()}`);
  const data = await res.json();
  creds.accessToken = data.access_token;
  creds.expiresAt = Date.now() + (data.expires_in ?? 3599) * 1000;
  return data.access_token;
}

async function gmail(config: Config, creds: UserCreds, path: string, init?: RequestInit, userId?: string): Promise<any> {
  const token = await getAccessToken(config, creds, userId);
  const res = await fetch(`${GMAIL_API}${path}`, {
    ...init,
    headers: { Authorization: `Bearer ${token}`, ...init?.headers },
  });
  if (!res.ok) throw new Error(`Gmail API ${res.status}: ${await res.text()}`);
  return res.json();
}

async function sendEmail(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { to, subject, body, cc, bcc, html } = input;
  const headers = [
    `To: ${to}`,
    cc && `Cc: ${cc}`,
    bcc && `Bcc: ${bcc}`,
    `Subject: ${subject}`,
    `Content-Type: ${html ? "text/html" : "text/plain"}; charset=UTF-8`,
  ].filter(Boolean).join("\r\n");
  const mime = `${headers}\r\n\r\n${body}`;

  const raw = btoa(unescape(encodeURIComponent(mime)))
    .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

  const result = await gmail(config, creds, "/messages/send", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ raw }),
  }, userId);
  return { messageId: result.id, threadId: result.threadId };
}

async function listEmails(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { query, maxResults = 10, labelIds, pageToken } = input ?? {};
  const params = new URLSearchParams({ maxResults: String(maxResults) });
  if (query) params.set("q", query);
  if (pageToken) params.set("pageToken", pageToken);
  if (labelIds?.length) for (const l of labelIds) params.append("labelIds", l);

  const list = await gmail(config, creds, `/messages?${params}`, undefined, userId);
  if (!list.messages?.length) return { messages: [], nextPageToken: null, resultSizeEstimate: 0 };

  return {
    messages: list.messages.map((m: any) => ({ id: m.id, threadId: m.threadId })),
    nextPageToken: list.nextPageToken ?? null,
    resultSizeEstimate: list.resultSizeEstimate ?? list.messages.length,
  };
}

async function getEmail(config: Config, creds: UserCreds, input: any, userId?: string) {
  return parseMessage(await gmail(config, creds, `/messages/${input.messageId}?format=full`, undefined, userId));
}

async function batchGetEmails(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { messageIds } = input;
  if (!messageIds?.length) return { messages: [] };

  const token = await getAccessToken(config, creds, userId);
  const auth = new google.auth.OAuth2();
  auth.setCredentials({ access_token: token });

  const fetchImpl = batchFetchImplementation({ maxBatchSize: 50 });
  const gmailClient = google.gmail({ version: "v1", auth, fetchImplementation: fetchImpl });

  const results = await Promise.allSettled(
    messageIds.map((id: string) =>
      gmailClient.users.messages.get({ userId: "me", id, format: "full" })
    )
  );

  const messages: any[] = [];
  for (const r of results) {
    if (r.status === "fulfilled" && r.value?.data) {
      messages.push(parseMessage(r.value.data));
    }
  }

  return { messages };
}

async function modifyEmail(config: Config, creds: UserCreds, input: any, userId?: string) {
  const body: any = {};
  if (input.addLabels?.length) body.addLabelIds = input.addLabels;
  if (input.removeLabels?.length) body.removeLabelIds = input.removeLabels;
  await gmail(config, creds, `/messages/${input.messageId}/modify`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }, userId);
  return { ok: true };
}

async function historyList(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { startHistoryId, maxResults = 100, pageToken, historyTypes = ["messageAdded", "messageDeleted"] } = input;
  if (!startHistoryId) throw new Error("startHistoryId is required");

  const params = new URLSearchParams({ startHistoryId, maxResults: String(maxResults) });
  if (pageToken) params.set("pageToken", pageToken);
  for (const t of historyTypes) params.append("historyTypes", t);

  const data = await gmail(config, creds, `/history?${params}`, undefined, userId);

  const messagesAdded: string[] = [];
  const messagesDeleted: string[] = [];
  for (const entry of data.history ?? []) {
    for (const m of entry.messagesAdded ?? []) if (m.message?.id) messagesAdded.push(m.message.id);
    for (const m of entry.messagesDeleted ?? []) if (m.message?.id) messagesDeleted.push(m.message.id);
  }

  return {
    messagesAdded,
    messagesDeleted,
    historyId: data.historyId ?? null,
    nextPageToken: data.nextPageToken ?? null,
  };
}

function htmlToText(html: string): string {
  const text = convert(html, { wordwrap: false, preserveNewlines: true }).trim();
  return text.replace(/ /g, " ").replace(/\n{3,}/g, "\n\n");
}

function parseMessage(msg: any) {
  const hdrs = msg.payload?.headers ?? [];
  const h = (n: string) => hdrs.find((h: any) => h.name.toLowerCase() === n.toLowerCase())?.value ?? "";
  const rawDate = h("Date");
  let isoDate = "";
  if (rawDate) {
    const parsed = new Date(rawDate);
    isoDate = isNaN(parsed.getTime()) ? rawDate : parsed.toISOString();
  }
  const rawBody = extractBody(msg.payload);
  const body = rawBody ? htmlToText(rawBody) : "";
  return {
    id: msg.id, threadId: msg.threadId, historyId: msg.historyId ?? null,
    headerMessageId: h("Message-ID") || h("Message-Id") || null,
    from: h("From"), to: h("To"), cc: h("Cc"), subject: h("Subject"),
    date: isoDate,
    snippet: msg.snippet, labelIds: msg.labelIds ?? [],
    body,
  };
}

function extractBody(payload: any): string {
  if (!payload) return "";
  if (payload.body?.data) return decodeBase64Url(payload.body.data);
  if (!payload.parts) return "";
  const part = payload.parts.find((p: any) => p.mimeType === "text/html")
    ?? payload.parts.find((p: any) => p.mimeType === "text/plain");
  return part?.body?.data ? decodeBase64Url(part.body.data) : "";
}

function decodeBase64Url(data: string): string {
  return decodeURIComponent(escape(atob(data.replace(/-/g, "+").replace(/_/g, "/"))));
}

const actions: Record<string, (c: Config, u: UserCreds, i: any, userId?: string) => Promise<any>> = {
  send_email: sendEmail, list_emails: listEmails, get_email: getEmail, batch_get_emails: batchGetEmails, modify_email: modifyEmail, history_list: historyList,
};

const rpcHandlers: Record<string, (params: any) => Promise<any>> = {
  __auth_start: authStart,
  __auth_callback: authCallback,

  async __integration(params) {
    const { action, input, config, userCredentials, userId } = params;
    const connected = isManaged(config) ? userCredentials?.managed : userCredentials?.refreshToken;
    if (!connected) throw new Error("user not connected — OAuth required");
    const handler = actions[action];
    if (!handler) throw new Error(`unknown action: ${action}`);
    return handler(config, userCredentials, input, userId);
  },

  async __webhook(params) {
    const { body } = params;
    const data = body?.message?.data;
    if (!data) return { skipped: true, reason: "no push data" };
    const decoded = JSON.parse(decodeBase64Url(data));
    if (!decoded.historyId) return { skipped: true, reason: "no historyId" };
    return { event: "push_notification", historyId: decoded.historyId };
  },
};

serve({ rpc: rpcHandlers });
