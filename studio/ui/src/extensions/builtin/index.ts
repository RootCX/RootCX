import { activate as explorer } from "./explorer";
import { activate as forge } from "./forge";
import { activate as welcome } from "./welcome";
import { activate as console } from "./console";
import { activate as output } from "./output";
import { activate as coreStatus } from "./core-status";

export function activateBuiltins() {
  explorer();
  forge();
  welcome();
  console();
  output();
  coreStatus();
}
