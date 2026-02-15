/** Mirrors rootcx-shared-types::OsStatus */
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
