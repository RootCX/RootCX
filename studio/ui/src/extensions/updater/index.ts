import { statusBar } from "@/core/studio";
import { UpdateStatus } from "./update-status";

export function activate() {
  statusBar.register("rootcx.updater", {
    alignment: "right",
    priority: 100,
    component: UpdateStatus,
  });
}
