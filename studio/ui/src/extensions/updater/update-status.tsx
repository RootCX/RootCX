import { useState, useEffect, useCallback } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { ask } from "@tauri-apps/plugin-dialog";
import { ArrowDownCircle } from "lucide-react";

const CHECK_INTERVAL_MS = 60 * 60 * 1000; // 1 hour

export function UpdateStatus() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [installing, setInstalling] = useState(false);

  const checkUpdate = useCallback(async () => {
    try {
      setUpdate(await check());
    } catch {
      // offline or endpoint unreachable — silent
    }
  }, []);

  useEffect(() => {
    checkUpdate();
    const id = setInterval(checkUpdate, CHECK_INTERVAL_MS);
    return () => clearInterval(id);
  }, [checkUpdate]);

  if (!update) return null;

  const install = async () => {
    setInstalling(true);
    try {
      await update.downloadAndInstall();
      const yes = await ask(`v${update.version} installed. Restart now?`, {
        title: "Update Ready",
        kind: "info",
      });
      if (yes) await relaunch();
    } catch {
      setInstalling(false);
    }
  };

  return (
    <button
      onClick={install}
      disabled={installing}
      className="flex items-center gap-1 px-2 text-xs text-blue-400 hover:text-blue-300 disabled:opacity-50"
    >
      <ArrowDownCircle className="h-3 w-3" />
      {installing ? "Installing..." : `v${update.version}`}
    </button>
  );
}
