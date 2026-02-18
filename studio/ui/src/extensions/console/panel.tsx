import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useProjectContext } from "@/components/layout/app-context";
import { XTERM_THEME } from "@/lib/xterm-theme";

export default function ConsolePanel() {
  const containerRef = useRef<HTMLDivElement>(null);
  const { projectPath } = useProjectContext();

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      theme: XTERM_THEME,
      fontFamily: '"JetBrains Mono", "Fira Code", ui-monospace, monospace',
      fontSize: 13,
      cursorBlink: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);

    requestAnimationFrame(() => fit.fit());

    const channel = new Channel<number[]>();
    channel.onmessage = (data) => term.write(new Uint8Array(data));

    invoke("spawn_terminal", {
      cwd: projectPath,
      rows: term.rows,
      cols: term.cols,
      channel,
    });

    term.onData((data) => invoke("terminal_write", { data }));

    const ro = new ResizeObserver(() => {
      fit.fit();
      invoke("terminal_resize", { rows: term.rows, cols: term.cols });
    });
    ro.observe(el);

    return () => {
      ro.disconnect();
      term.dispose();
    };
  }, [projectPath]);

  return <div ref={containerRef} className="h-full w-full pt-1 pl-2" />;
}
