/** Mirrors rootcx-shared-types::OsStatus */
export interface OsStatus {
  kernel: KernelStatus;
  postgres: PostgresStatus;
  forge: ForgeStatus;
}

export interface ForgeStatus {
  state: ServiceState;
  port: number | null;
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

/** Mirrors rootcx-shared-types::InstalledApp */
export interface InstalledApp {
  id: string;
  name: string;
  version: string;
  status: string;
  entities: string[];
}
