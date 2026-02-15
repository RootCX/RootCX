import { statusBar } from "@/core/studio";
import { ServiceStatus } from "./service-status";

export function activate() {
  statusBar.register("rootcx.services", {
    alignment: "left",
    priority: 0,
    component: ServiceStatus,
  });
}
