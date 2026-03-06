import { activate as commandPalette } from "./command-palette";
import { activate as project } from "./project";
import { activate as explorer } from "./explorer";
import { activate as forge } from "./forge";
import { activate as welcome } from "./welcome";
import { activate as console } from "./console";
import { activate as output } from "./output";
import { activate as coreStatus } from "./core-status";
import { activate as run } from "./run";
import { activate as editor } from "./editor";
import { activate as settings } from "./settings";
import { activate as database } from "./database";
import { activate as security } from "./security";
import { activate as bundle } from "./bundle";
import { activate as workers } from "./workers";
import { activate as integrations } from "./integrations";
import { activate as mcpServers } from "./mcp-servers";

export function activateBuiltins() {
  commandPalette();
  project();
  explorer();
  welcome();
  forge();
  console();
  output();
  coreStatus();
  run();
  bundle();
  editor();
  settings();
  database();
  security();
  workers();
  integrations();
  mcpServers();
}
