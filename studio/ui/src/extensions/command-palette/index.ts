import { commands } from "@/core/studio";
import { openPalette } from "./store";

export function activate() {
  commands.register("commandPalette.open", {
    title: "Command Palette",
    category: "View",
    keybinding: "Mod+Shift+P",
    handler: () => openPalette(),
  });
}
