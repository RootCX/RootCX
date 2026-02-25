import type { EntitySchema } from "../runner.js";

export interface ToolContext {
    appId: string;
    runtimeUrl: string;
    authToken: string;
    dataContract: EntitySchema[];
}
