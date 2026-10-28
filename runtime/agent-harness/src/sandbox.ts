import { StoreBackend, type ExecuteResponse } from "deepagents";
import type { BaseStore } from "@langchain/langgraph-checkpoint";
import type { Attachment, Bridge } from "./harness";

export class GovernedSandbox extends StoreBackend {
  readonly id = "rootcx-governed-workspace";
  constructor(store: BaseStore, readonly bridge: Bridge, readonly attachments: Attachment[]) { super({ store, fileFormat: "v2" }); }
  async execute(command: string): Promise<ExecuteResponse> {
    const files: { path: string; text?: string; file_id?: string }[] = this.attachments.map(a => ({
      path: `attachments/${a.file_id}/${a.name.replace(/[^\p{L}\p{N}._-]/gu, "_").slice(-180)}`, file_id: a.file_id,
    }));
    const listing = await this.glob("/workspace/**");
    for (const file of listing.files ?? []) {
      if (file.is_dir) continue;
      const result = await this.readRaw(file.path);
      if (result.data && typeof result.data.content === "string") files.push({ path: file.path.slice(1), text: result.data.content });
    }
    const prior = (await this.readRaw("/workspace/artifacts.json")).data;
    if (prior && typeof prior.content === "string") for (const artifact of JSON.parse(prior.content))
      files.push({ path: artifact.path, file_id: artifact.file_id });
    let result;
    try { result = await this.bridge.callTool("execute_sandbox", { command, files }); }
    catch (error: any) { return { output: String(error.message ?? error), exitCode: 1, truncated: false }; }
    if (result.artifacts?.length) {
      const existing = (await this.readRaw("/workspace/artifacts.json")).data;
      const content = [...(existing && typeof existing.content === "string" ? JSON.parse(existing.content) : []), ...result.artifacts];
      await this.uploadFiles([["/workspace/artifacts.json", Buffer.from(JSON.stringify(content, null, 2))]]);
    }
    return { output: result.output + (result.artifacts?.length ? `\nGenerated files (use file_id with app actions):\n${JSON.stringify(result.artifacts)}` : ""),
      exitCode: result.exit_code, truncated: result.truncated };
  }
}
