import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useProjectContext } from "@/components/layout/app-context";

const THEME = {
  background: "#0d0d0d",
  foreground: "#d4d4d8",
  cursor: "#3b82f6",
  selectionBackground: "#3b82f680",
  black: "#1e1e2e",
  red: "#f38ba8",
  green: "#a6e3a1",
  yellow: "#f9e2af",
  blue: "#89b4fa",
  magenta: "#cba6f7",
  cyan: "#94e2d5",
  white: "#cdd6f4",
  brightBlack: "#585b70",
  brightRed: "#f38ba8",
  brightGreen: "#a6e3a1",
  brightYellow: "#f9e2af",
  brightBlue: "#89b4fa",
  brightMagenta: "#cba6f7",
  brightCyan: "#94e2d5",
  brightWhite: "#ffffff",
};

export default function ConsolePanel() {
  const containerRef = useRef<HTMLDivElement>(null);
  const { projectPath } = useProjectContext();

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const term = new Terminal({
      theme: THEME,
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
