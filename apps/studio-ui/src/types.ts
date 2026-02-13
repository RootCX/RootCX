/** Mirrors rootcx-shared-types::OsStatus */
export interface OsStatus {
  kernel: KernelStatus;
  postgres: PostgresStatus;
}

export interface KernelStatus {
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
