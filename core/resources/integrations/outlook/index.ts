/// <reference path="../rootcx-worker.d.ts" />
const GRAPH_API = "https://graph.microsoft.com/v1.0/me";
const AUTH_URL = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN_URL = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const SCOPES = "offline_access Mail.ReadWrite Mail.Send";

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

  const url = new URL(AUTH_URL);
  url.searchParams.set("client_id", config.clientId!);
  url.searchParams.set("redirect_uri", callbackUrl);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("scope", SCOPES);
  url.searchParams.set("prompt", "consent");
  if (state) url.searchParams.set("state", state);
  return { type: "redirect", url: url.toString() };
}

async function authCallback(params: any) {
  const { config, query } = params;

  if (isManaged(config) || query.code === "MANAGED_OK") {
    return { credentials: { managed: true } };
  }

  const res = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      code: query.code,
      client_id: config.clientId!,
      client_secret: config.clientSecret!,
      redirect_uri: query.redirect_uri ?? params.callbackUrl ?? "",
      grant_type: "authorization_code",
      scope: SCOPES,
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
  const res = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: config.clientId!,
      client_secret: config.clientSecret!,
      refresh_token: creds.refreshToken!,
      grant_type: "refresh_token",
      scope: SCOPES,
    }),
  });
  if (!res.ok) throw new Error(`token refresh failed: ${await res.text()}`);
  const data = await res.json();
  creds.accessToken = data.access_token;
  creds.expiresAt = Date.now() + (data.expires_in ?? 3599) * 1000;
  return data.access_token;
}

async function graph(config: Config, creds: UserCreds, path: string, init?: RequestInit, userId?: string): Promise<Response> {
  const token = await getAccessToken(config, creds, userId);
  return fetch(`${GRAPH_API}${path}`, {
    ...init,
    headers: { Authorization: `Bearer ${token}`, ...init?.headers },
  });
}

async function graphJson(config: Config, creds: UserCreds, path: string, init?: RequestInit, userId?: string): Promise<any> {
  const res = await graph(config, creds, path, init, userId);
  if (!res.ok) throw new Error(`Graph API ${res.status}: ${await res.text()}`);
  return res.json();
}

const fmtAddr = (r: any) => r?.emailAddress?.address
  ? (r.emailAddress.name ? `${r.emailAddress.name} <${r.emailAddress.address}>` : r.emailAddress.address)
  : "";

// ─── Actions ────────────────────────────────────────────────────────────────

async function sendEmail(config: Config, creds: UserCreds, input: any, userId?: string) {
  const toList = input.to.split(",").map((a: string) => ({ emailAddress: { address: a.trim() } }));
  const message: any = {
    subject: input.subject,
    body: { contentType: input.html ? "HTML" : "Text", content: input.body },
    toRecipients: toList,
  };
  if (input.cc) message.ccRecipients = input.cc.split(",").map((a: string) => ({ emailAddress: { address: a.trim() } }));
  if (input.bcc) message.bccRecipients = input.bcc.split(",").map((a: string) => ({ emailAddress: { address: a.trim() } }));

  const res = await graph(config, creds, "/sendMail", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ message, saveToSentItems: input.saveToSentItems ?? true }),
  }, userId);

  if (!res.ok) throw new Error(`sendMail ${res.status}: ${await res.text()}`);
  return { ok: true };
}

async function listEmails(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { folder = "Inbox", top = 10, filter, search } = input ?? {};

  const params = new URLSearchParams();
  params.set("$top", String(top));
  params.set("$select", "id,conversationId,internetMessageId,subject,from,toRecipients,ccRecipients,receivedDateTime,isRead,bodyPreview");
  params.set("$orderby", "receivedDateTime desc");
  if (filter) params.set("$filter", filter);
  if (search) params.set("$search", `"${search}"`);

  const data = await graphJson(config, creds, `/mailFolders/${folder}/messages?${params}`, undefined, userId);

  return {
    messages: (data.value ?? []).map((m: any) => ({
      id: m.id,
      conversationId: m.conversationId,
      internetMessageId: m.internetMessageId ?? null,
      subject: m.subject,
      from: fmtAddr(m.from),
      to: (m.toRecipients ?? []).map(fmtAddr).join(", "),
      cc: (m.ccRecipients ?? []).map(fmtAddr).join(", "),
      receivedDateTime: m.receivedDateTime,
      isRead: m.isRead,
      bodyPreview: m.bodyPreview,
    })),
    nextLink: data["@odata.nextLink"] ?? null,
  };
}

async function getEmail(config: Config, creds: UserCreds, input: any, userId?: string) {
  const msg = await graphJson(
    config, creds,
    `/messages/${input.messageId}?$select=id,conversationId,internetMessageId,subject,from,toRecipients,ccRecipients,receivedDateTime,isRead,importance,hasAttachments,body,flag`,
    { headers: { "Prefer": 'outlook.body-content-type="text"' } },
    userId,
  );
  return {
    id: msg.id,
    conversationId: msg.conversationId,
    internetMessageId: msg.internetMessageId ?? null,
    subject: msg.subject,
    from: fmtAddr(msg.from),
    to: (msg.toRecipients ?? []).map(fmtAddr).join(", "),
    cc: (msg.ccRecipients ?? []).map(fmtAddr).join(", "),
    receivedDateTime: msg.receivedDateTime,
    isRead: msg.isRead,
    importance: msg.importance,
    hasAttachments: msg.hasAttachments,
    body: msg.body?.content ?? "",
    flag: msg.flag ?? null,
  };
}

async function batchGetEmails(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { messageIds } = input;
  if (!messageIds?.length) return { messages: [] };

  const messages: any[] = [];

  // MS Graph batch max = 20 requests
  for (let i = 0; i < messageIds.length; i += 20) {
    const chunk = messageIds.slice(i, i + 20);
    const token = await getAccessToken(config, creds, userId);

    const batchReq = {
      requests: chunk.map((id: string, idx: number) => ({
        id: String(idx),
        method: "GET",
        url: `/me/messages/${id}?$select=id,conversationId,internetMessageId,subject,from,toRecipients,ccRecipients,receivedDateTime,body,isRead`,
        headers: { "Prefer": 'outlook.body-content-type="text"' },
      })),
    };

    const res = await fetch("https://graph.microsoft.com/v1.0/$batch", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(batchReq),
    });

    if (!res.ok) throw new Error(`$batch ${res.status}: ${await res.text()}`);
    const data = await res.json();

    for (const r of data.responses ?? []) {
      if (r.status === 200 && r.body) {
        const m = r.body;
        messages.push({
          id: m.id,
          conversationId: m.conversationId,
          internetMessageId: m.internetMessageId ?? null,
          headerMessageId: m.internetMessageId ?? null,
          subject: m.subject,
          from: fmtAddr(m.from),
          to: (m.toRecipients ?? []).map(fmtAddr).join(", "),
          cc: (m.ccRecipients ?? []).map(fmtAddr).join(", "),
          date: m.receivedDateTime,
          body: m.body?.content ?? "",
          labelIds: m.isRead ? [] : ["UNREAD"],
          threadId: m.conversationId ?? "",
        });
      }
    }
  }

  return { messages };
}

async function deltaList(config: Config, creds: UserCreds, input: any, userId?: string) {
  const { deltaLink, folder = "Inbox", top = 100 } = input;

  let url: string;
  if (deltaLink) {
    url = deltaLink;
  } else {
    const params = new URLSearchParams();
    params.set("$select", "id,internetMessageId,receivedDateTime");
    params.set("$top", String(top));
    url = `${GRAPH_API}/mailFolders/${folder}/messages/delta?${params}`;
  }

  const token = await getAccessToken(config, creds, userId);
  const res = await fetch(url, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) throw new Error(`delta ${res.status}: ${await res.text()}`);
  const data = await res.json();

  const messagesAdded: string[] = [];
  const messagesDeleted: string[] = [];

  for (const item of data.value ?? []) {
    if (item["@removed"]) {
      messagesDeleted.push(item.id);
    } else {
      messagesAdded.push(item.id);
    }
  }

  return {
    messagesAdded,
    messagesDeleted,
    nextLink: data["@odata.nextLink"] ?? null,
    deltaLink: data["@odata.deltaLink"] ?? null,
  };
}

async function modifyEmail(config: Config, creds: UserCreds, input: any, userId?: string) {
  const patch: any = {};
  if (input.isRead !== undefined) patch.isRead = input.isRead;
  if (input.flag) patch.flag = { flagStatus: input.flag };
  if (input.importance) patch.importance = input.importance;
  if (input.categories) patch.categories = input.categories;

  await graphJson(config, creds, `/messages/${input.messageId}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(patch),
  }, userId);
  return { ok: true };
}

const actions: Record<string, (c: Config, u: UserCreds, i: any, userId?: string) => Promise<any>> = {
  send_email: sendEmail,
  list_emails: listEmails,
  get_email: getEmail,
  batch_get_emails: batchGetEmails,
  delta_list: deltaList,
  modify_email: modifyEmail,
};

serve({
  rpc: {
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
      if (!body?.value?.length) return { skipped: true, reason: "no change notifications" };
      return {
        event: "mail_notification",
        changes: body.value.map((n: any) => ({
          changeType: n.changeType,
          resource: n.resource,
          subscriptionId: n.subscriptionId,
          resourceData: n.resourceData,
        })),
      };
    },
  },
});
