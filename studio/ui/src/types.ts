export interface OsStatus {
  runtime: RuntimeStatus;
  postgres: PostgresStatus;
  forge: ForgeStatus;
}

export interface ForgeStatus {
  state: ServiceState;
  port: number | null;
}

export interface RuntimeStatus {
  version: string;
  state: ServiceState;
}

export interface PostgresStatus {
  state: ServiceState;
  port: number | null;
  data_dir: string | null;
}

export type ServiceState =
  | "online"
  | "offline"
  | "starting"
  | "stopping"
  | "error";

export interface SchemaChange {
  entity: string;
  change_type: string;
  column: string;
  detail: string | null;
}

export interface SchemaVerification {
  compliant: boolean;
  changes: SchemaChange[];
}

export interface AgentMessage {
  role: "user" | "assistant" | "system";
  content: string;
  type?: "approval" | "tool_start" | "tool_done";
  meta?: Record<string, unknown>;
}
