export type ObjectValue = Record<string, any>;
export type Schema = Record<string, unknown>;
export type Effect = 'read' | 'write';
export type Credential = { accessToken: string; instanceUrl?: string; [field: string]: unknown };
export type Request = {
  service?: string;
  path: string; method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  query?: Record<string, unknown>; json?: unknown; form?: Record<string, unknown>;
  headers?: Record<string, string>; body?: BodyInit; response?: 'json' | 'binary';
};
export interface Context {
  request<T = ObjectValue>(request: Request): Promise<T>;
}
export interface Action {
  name: string; description: string; effect: Effect;
  input: Schema; output?: Schema;
  run(context: Context, input: ObjectValue): Promise<unknown>;
}
export interface OAuth {
  authorizeUrl: string; tokenUrl: string; scopes: string[];
  scopeSeparator?: ' ' | ',';
  pkce?: boolean; clientAuth?: 'basic' | 'body'; tokenEncoding?: 'json' | 'form';
  authorizationParams?: Record<string, string>;
  account(token: ObjectValue, context: Context): Promise<{ id: string; label: string }>;
}
export interface Plugin {
  protocol: 1; id: string; name: string; version: string; description: string;
  origin: string | ((credential: Credential) => string);
  services?: Record<string, string>;
  headers?: Record<string, string>;
  credentialSchema?: Schema;
  // Reviewed provider hook for API keys, Basic, HMAC signatures or other wire auth.
  authorize?(credential: Credential, request: { url: URL; method: string; body?: BodyInit }): Record<string, string> | Promise<Record<string, string>>;
  requestsPerSecond: number;
  oauth?: OAuth;
  tokenAuth?: { label: string; helpUrl: string };
  verify(context: Context): Promise<{ id: string; label: string }>;
  actions: Record<string, Action>;
  discovery?: { action: string; itemsPath: string; idPath: string; labelPath: string };
}
