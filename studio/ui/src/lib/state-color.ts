import type { ServiceState } from "@/types";

export function stateColor(state: ServiceState): string {
  switch (state) {
    case "online":
      return "bg-green-500";
    case "starting":
      return "bg-yellow-500 animate-[pulse-dot_1.5s_infinite]";
    case "stopping":
      return "bg-orange-500 animate-[pulse-dot_1.5s_infinite]";
    case "error":
      return "bg-red-500";
    default:
      return "bg-gray-500";
  }
}
