const GMAIL_API = "https://www.googleapis.com/gmail/v1/users/me";
const GOOGLE_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
const SCOPES = "https://www.googleapis.com/auth/gmail.modify";

interface Config { clientId: string; clientSecret: string }
interface UserCreds { refreshToken: string; accessToken?: string; expiresAt?: number }

async function authStart(params: any) {
  const { config, callbackUrl, state } = params;
  const url = new URL(GOOGLE_AUTH_URL);
  url.searchParams.set("client_id", config.clientId);
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
  const res = await fetch(GOOGLE_TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      code: query.code,
      client_id: config.clientId,
      client_secret: config.clientSecret,
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

async function getAccessToken(config: Config, creds: UserCreds): Promise<string> {
  if (creds.accessToken && creds.expiresAt && Date.now() < creds.expiresAt - 30_000) {
    return creds.accessToken;
  }
  const res = await fetch(GOOGLE_TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: config.clientId,
      client_secret: config.clientSecret,
      refresh_token: creds.refreshToken,
      grant_type: "refresh_token",
    }),
  });
  if (!res.ok) throw new Error(`token refresh failed: ${await res.text()}`);
  const data = await res.json();
  return data.access_token;
}

async function gmail(config: Config, creds: UserCreds, path: string, init?: RequestInit): Promise<any> {
  const token = await getAccessToken(config, creds);
  const res = await fetch(`${GMAIL_API}${path}`, {
    ...init,
    headers: { Authorization: `Bearer ${token}`, ...init?.headers },
  });
  if (!res.ok) throw new Error(`Gmail API ${res.status}: ${await res.text()}`);
  return res.json();
}

async function sendEmail(config: Config, creds: UserCreds, input: any) {
  const { to, subject, body, cc, bcc, html } = input;
  const mime = [
    `To: ${to}`,
    cc ? `Cc: ${cc}` : "",
    bcc ? `Bcc: ${bcc}` : "",
    `Subject: ${subject}`,
    `Content-Type: ${html ? "text/html" : "text/plain"}; charset=UTF-8`,
    "",
    body,
  ].filter(Boolean).join("\r\n");

  const raw = btoa(unescape(encodeURIComponent(mime)))
    .replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

  const result = await gmail(config, creds, "/messages/send", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ raw }),
  });
  return { messageId: result.id, threadId: result.threadId };
}

async function listEmails(config: Config, creds: UserCreds, input: any) {
  const { query, maxResults = 10, labelIds = ["INBOX"] } = input ?? {};
  const params = new URLSearchParams({ maxResults: String(maxResults) });
  if (query) params.set("q", query);
  for (const l of labelIds) params.append("labelIds", l);

  const list = await gmail(config, creds, `/messages?${params}`);
  if (!list.messages?.length) return { messages: [], resultSizeEstimate: 0 };

  return {
    messages: list.messages.map((m: any) => ({ id: m.id, threadId: m.threadId })),
    resultSizeEstimate: list.resultSizeEstimate ?? list.messages.length,
  };
}

async function getEmail(config: Config, creds: UserCreds, input: any) {
  return parseMessage(await gmail(config, creds, `/messages/${input.messageId}?format=full`));
}

async function modifyEmail(config: Config, creds: UserCreds, input: any) {
  const body: any = {};
  if (input.addLabels?.length) body.addLabelIds = input.addLabels;
  if (input.removeLabels?.length) body.removeLabelIds = input.removeLabels;
  await gmail(config, creds, `/messages/${input.messageId}/modify`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return { ok: true };
}

function parseMessage(msg: any) {
  const hdrs = msg.payload?.headers ?? [];
  const h = (n: string) => hdrs.find((h: any) => h.name.toLowerCase() === n.toLowerCase())?.value ?? "";
  return {
    id: msg.id, threadId: msg.threadId,
    from: h("From"), to: h("To"), subject: h("Subject"), date: h("Date"),
    snippet: msg.snippet, labelIds: msg.labelIds ?? [],
    body: extractBody(msg.payload),
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

const actions: Record<string, (c: Config, u: UserCreds, i: any) => Promise<any>> = {
  send_email: sendEmail, list_emails: listEmails, get_email: getEmail, modify_email: modifyEmail,
};

const rpcHandlers: Record<string, (params: any) => Promise<any>> = {
  __auth_start: authStart,
  __auth_callback: authCallback,

  async __integration(params) {
    const { action, input, config, userCredentials } = params;
    if (!userCredentials?.refreshToken) throw new Error("user not connected — OAuth required");
    const handler = actions[action];
    if (!handler) throw new Error(`unknown action: ${action}`);
    return handler(config, userCredentials, input);
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

const send = (msg: Record<string, unknown>) =>
  process.stdout.write(JSON.stringify(msg) + "\n");

let buffer = "";
process.stdin.setEncoding("utf-8");
process.stdin.on("data", (chunk: string) => {
  buffer += chunk;
  let nl: number;
  while ((nl = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;

    const msg = JSON.parse(line);
    switch (msg.type) {
      case "discover":
        send({ type: "discover", methods: Object.keys(rpcHandlers) });
        break;
      case "rpc":
        handleRpc(msg);
        break;
      case "shutdown":
        process.exit(0);
    }
  }
});

async function handleRpc(msg: any) {
  const handler = rpcHandlers[msg.method];
  if (!handler) {
    send({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` });
    return;
  }
  try {
    send({ type: "rpc_response", id: msg.id, result: await handler(msg.params) });
  } catch (e: any) {
    send({ type: "rpc_response", id: msg.id, error: e.message });
  }
}
