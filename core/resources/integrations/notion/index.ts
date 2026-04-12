/// <reference path="../rootcx-worker.d.ts" />
const NOTION_API = "https://api.notion.com/v1";
const NOTION_VERSION = "2026-03-11";

interface Config { apiToken?: string }

async function notion(config: Config, path: string, init?: RequestInit): Promise<any> {
  if (!config.apiToken) throw new Error("NOTION_API_TOKEN not configured");
  const res = await fetch(`${NOTION_API}${path}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${config.apiToken}`,
      "Notion-Version": NOTION_VERSION,
      "Content-Type": "application/json",
      ...init?.headers,
    },
  });
  if (!res.ok) throw new Error(`Notion API ${res.status}: ${await res.text()}`);
  return res.json();
}

function extractTitle(obj: any): string {
  const title = obj.title ?? Object.values(obj.properties ?? {}).find((p: any) => p.type === "title")?.title;
  return title?.map((t: any) => t.plain_text).join("") ?? "";
}

function simplifyProperties(properties: any): Record<string, any> {
  const out: Record<string, any> = {};
  for (const [name, prop] of Object.entries(properties) as any[])
    out[name] = simplifyPropertyValue(prop);
  return out;
}

function simplifyPropertyValue(prop: any): any {
  switch (prop.type) {
    case "title": return prop.title?.map((t: any) => t.plain_text).join("") ?? "";
    case "rich_text": return prop.rich_text?.map((t: any) => t.plain_text).join("") ?? "";
    case "number": return prop.number;
    case "select": return prop.select?.name ?? null;
    case "multi_select": return prop.multi_select?.map((s: any) => s.name) ?? [];
    case "date": return prop.date;
    case "checkbox": return prop.checkbox;
    case "url": return prop.url;
    case "email": return prop.email;
    case "phone_number": return prop.phone_number;
    case "formula": return prop.formula?.[prop.formula?.type];
    case "relation": return prop.relation?.map((r: any) => r.id) ?? [];
    case "rollup": return prop.rollup?.[prop.rollup?.type];
    case "people": return prop.people?.map((p: any) => p.name ?? p.id) ?? [];
    case "created_time": return prop.created_time;
    case "last_edited_time": return prop.last_edited_time;
    case "status": return prop.status?.name ?? null;
    default: return null;
  }
}

function buildPropertyValue(key: string, value: any, schema: Record<string, any>): any {
  const propSchema = schema[key];
  if (!propSchema) return null;
  switch (propSchema.type) {
    case "title": return { title: [{ text: { content: String(value) } }] };
    case "rich_text": return { rich_text: [{ text: { content: String(value) } }] };
    case "number": return { number: Number(value) };
    case "select": return { select: { name: String(value) } };
    case "multi_select": return { multi_select: (Array.isArray(value) ? value : [value]).map((v: any) => ({ name: String(v) })) };
    case "date": return { date: typeof value === "string" ? { start: value } : value };
    case "checkbox": return { checkbox: Boolean(value) };
    case "url": return { url: String(value) };
    case "email": return { email: String(value) };
    case "phone_number": return { phone_number: String(value) };
    case "status": return { status: { name: String(value) } };
    default: return null;
  }
}

function toNotionProperties(input: Record<string, any>, schema: Record<string, any>): Record<string, any> {
  const out: Record<string, any> = {};
  for (const [key, value] of Object.entries(input)) {
    const built = buildPropertyValue(key, value, schema);
    if (built) out[key] = built;
  }
  return out;
}

function simplifySchema(properties: any): Record<string, any> {
  const out: Record<string, any> = {};
  for (const [name, prop] of Object.entries(properties) as any[])
    out[name] = { id: prop.id, type: prop.type, ...(prop[prop.type] && Object.keys(prop[prop.type]).length ? { config: prop[prop.type] } : {}) };
  return out;
}

function simplifyResult(item: any): any {
  return {
    id: item.id,
    title: extractTitle(item),
    url: item.url,
    objectType: item.object,
    properties: item.properties ? simplifyProperties(item.properties) : undefined,
    lastEditedAt: item.last_edited_time,
  };
}

async function resolveDataSourceId(config: Config, databaseId: string): Promise<string> {
  const db = await notion(config, `/databases/${databaseId}`);
  const ds = db.data_sources?.[0];
  if (!ds?.id) throw new Error(`no data source found for database ${databaseId}`);
  return ds.id;
}

async function getDataSourceSchema(config: Config, dataSourceId: string): Promise<Record<string, any>> {
  const ds = await notion(config, `/data_sources/${dataSourceId}`);
  return ds.properties ?? {};
}

async function search(config: Config, input: any) {
  const body: any = {};
  if (input.query) body.query = input.query;
  if (input.filter) body.filter = { value: input.filter, property: "object" };
  if (input.pageSize) body.page_size = input.pageSize;
  if (input.startCursor) body.start_cursor = input.startCursor;
  const data = await notion(config, "/search", { method: "POST", body: JSON.stringify(body) });
  return { results: data.results.map(simplifyResult), hasMore: data.has_more, nextCursor: data.next_cursor };
}

async function getPage(config: Config, input: any) {
  const [page, md] = await Promise.all([
    notion(config, `/pages/${input.pageId}`),
    notion(config, `/pages/${input.pageId}/markdown`),
  ]);
  return {
    id: page.id,
    title: extractTitle(page),
    url: page.url,
    markdown: md.markdown ?? "",
    properties: simplifyProperties(page.properties),
    lastEditedAt: page.last_edited_time,
  };
}

async function createPage(config: Config, input: any) {
  const body: any = {};

  if (input.parentType === "database") {
    const dsId = await resolveDataSourceId(config, input.parentId);
    const schema = await getDataSourceSchema(config, dsId);
    body.parent = { data_source_id: dsId };
    body.properties = input.properties ? toNotionProperties(input.properties, schema) : {};
    const titleKey = Object.entries(schema).find(([, v]: any) => v.type === "title")?.[0];
    if (titleKey && input.title) body.properties[titleKey] = { title: [{ text: { content: input.title } }] };
  } else {
    body.parent = { page_id: input.parentId };
    body.properties = { title: { title: [{ text: { content: input.title ?? "Untitled" } }] } };
  }

  if (input.markdown) body.markdown = input.markdown;

  const page = await notion(config, "/pages", { method: "POST", body: JSON.stringify(body) });
  return { id: page.id, url: page.url };
}

async function patchProperties(config: Config, pageId: string, properties: Record<string, any>): Promise<any> {
  const page = await notion(config, `/pages/${pageId}`);
  const parentDsId = page.parent?.data_source_id;
  let props = properties;
  if (parentDsId) {
    const schema = await getDataSourceSchema(config, parentDsId);
    props = toNotionProperties(properties, schema);
  }
  return notion(config, `/pages/${pageId}`, { method: "PATCH", body: JSON.stringify({ properties: props }) });
}

async function updatePage(config: Config, input: any) {
  const ops: Promise<any>[] = [];
  if (input.properties) ops.push(patchProperties(config, input.pageId, input.properties));
  if (input.markdown) ops.push(notion(config, `/pages/${input.pageId}/markdown`, {
    method: "PATCH",
    body: JSON.stringify({ type: "replace_content", replace_content: { new_str: input.markdown } }),
  }));

  if (!ops.length) return { id: input.pageId, url: null, lastEditedAt: null };
  const results = await Promise.all(ops);
  const last = results[0];
  return { id: last.id ?? input.pageId, url: last.url, lastEditedAt: last.last_edited_time };
}

async function trashPage(config: Config, input: any) {
  await notion(config, `/pages/${input.pageId}`, {
    method: "PATCH",
    body: JSON.stringify({ in_trash: true }),
  });
  return { ok: true };
}

async function listDatabases(config: Config, input: any) {
  const body: any = { filter: { value: "data_source", property: "object" }, page_size: input.pageSize ?? 10 };
  if (input.startCursor) body.start_cursor = input.startCursor;
  const data = await notion(config, "/search", { method: "POST", body: JSON.stringify(body) });
  return {
    databases: data.results.map((ds: any) => ({
      id: ds.id,
      title: extractTitle(ds),
      description: ds.description?.map((t: any) => t.plain_text).join("") ?? "",
      url: ds.url,
      propertyCount: Object.keys(ds.properties ?? {}).length,
    })),
    hasMore: data.has_more,
    nextCursor: data.next_cursor,
  };
}

async function getDatabaseSchema(config: Config, input: any) {
  const ds = await notion(config, `/data_sources/${input.databaseId}`);
  return {
    id: ds.id,
    title: extractTitle(ds),
    properties: simplifySchema(ds.properties ?? {}),
  };
}

async function queryDatabase(config: Config, input: any) {
  const body: any = { page_size: input.pageSize ?? 100 };
  if (input.filter) body.filter = input.filter;
  if (input.sorts) body.sorts = input.sorts;
  if (input.startCursor) body.start_cursor = input.startCursor;
  const data = await notion(config, `/data_sources/${input.databaseId}/query`, { method: "POST", body: JSON.stringify(body) });
  return {
    results: data.results.map((page: any) => ({
      id: page.id,
      url: page.url,
      properties: simplifyProperties(page.properties),
      lastEditedAt: page.last_edited_time,
    })),
    hasMore: data.has_more,
    nextCursor: data.next_cursor,
  };
}

async function createEntry(config: Config, input: any) {
  const dsId = await resolveDataSourceId(config, input.databaseId);
  const schema = await getDataSourceSchema(config, dsId);
  const body: any = {
    parent: { data_source_id: dsId },
    properties: toNotionProperties(input.properties, schema),
  };
  if (input.markdown) body.markdown = input.markdown;
  const page = await notion(config, "/pages", { method: "POST", body: JSON.stringify(body) });
  return { id: page.id, url: page.url };
}

async function updateEntry(config: Config, input: any) {
  const updated = await patchProperties(config, input.pageId, input.properties);
  return { id: updated.id, url: updated.url, lastEditedAt: updated.last_edited_time };
}

async function addComment(config: Config, input: any) {
  const body = {
    parent: { type: "page_id", page_id: input.pageId },
    rich_text: [{ text: { content: input.text } }],
  };
  const comment = await notion(config, "/comments", { method: "POST", body: JSON.stringify(body) });
  return { id: comment.id };
}

const actions: Record<string, (c: Config, i: any) => Promise<any>> = {
  search, get_page: getPage, create_page: createPage, update_page: updatePage, trash_page: trashPage,
  list_databases: listDatabases, get_database_schema: getDatabaseSchema, query_database: queryDatabase,
  create_entry: createEntry, update_entry: updateEntry, add_comment: addComment,
};

const rpcHandlers: Record<string, (params: any) => Promise<any>> = {
  async __integration(params) {
    const { action, input, config } = params;
    const handler = actions[action];
    if (!handler) throw new Error(`unknown action: ${action}`);
    return handler(config, input ?? {});
  },

  async __webhook(params) {
    const { body } = params;
    if (!body?.type) return { skipped: true, reason: "no event type" };
    return { event: body.type, data: body };
  },
};

serve({ rpc: rpcHandlers });
