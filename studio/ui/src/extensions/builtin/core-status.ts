import { statusBar } from "../studio";
import { ServiceStatus } from "@/components/layout/service-status";

export function activate() {
  statusBar.register("rootcx.services", {
    alignment: "left",
    priority: 0,
    component: ServiceStatus,
  });
}
