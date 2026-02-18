import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import { XTERM_THEME } from "@/lib/xterm-theme";

export default function OutputPanel() {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      theme: XTERM_THEME,
      fontFamily: '"JetBrains Mono", "Fira Code", ui-monospace, monospace',
      fontSize: 13,
      disableStdin: true,
      cursorBlink: false,
      cursorInactiveStyle: "none",
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    requestAnimationFrame(() => fit.fit());

    const listeners = Promise.all([
      listen("run-started", () => {
        term.clear();
        term.reset();
      }),
      listen<string>("run-output", (e) => term.write(e.payload)),
      listen<number | null>("run-exited", (e) => {
        const msg =
          e.payload != null
            ? `\r\n[process exited with code ${e.payload}]`
            : "\r\n[process exited]";
        term.write(msg);
      }),
    ]);

    const ro = new ResizeObserver(() => fit.fit());
    ro.observe(el);

    return () => {
      listeners.then((fns) => fns.forEach((fn) => fn()));
      ro.disconnect();
      term.dispose();
    };
  }, []);

  return <div ref={containerRef} className="h-full w-full pt-1 pl-2" />;
}
