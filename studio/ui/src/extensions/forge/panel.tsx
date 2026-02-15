import { useState, useRef, useEffect, useSyncExternalStore } from "react";
import { subscribe, getSnapshot, sendMessage, stopBuild, type ForgeMessage } from "./store";
import { useProjectContext } from "@/components/layout/app-context";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

function Message({ msg }: { msg: ForgeMessage }) {
  return (
    <div
      className={cn(
        "rounded-md px-3 py-2 text-xs leading-relaxed",
        msg.role === "user" && "self-end max-w-[80%] border border-blue-600 bg-blue-950",
        msg.role === "assistant" && "whitespace-pre-wrap break-words border border-border bg-background",
        msg.role === "status" && "font-mono text-muted-foreground py-0.5 px-0",
      )}
    >
      {msg.role !== "status" && (
        <span className="mb-1 block text-[10px] font-semibold uppercase tracking-wider text-primary">
          {msg.role === "user" ? "You" : "Forge"}
        </span>
      )}
      <span>{msg.content}</span>
    </div>
  );
}

export default function ForgePanel() {
  const { messages, thinking, toolCalls, files, errors, isStreaming } =
    useSyncExternalStore(subscribe, getSnapshot);
  const { projectPath } = useProjectContext();
  const [input, setInput] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, thinking]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!input.trim() || !projectPath || isStreaming) return;
    sendMessage(input.trim(), projectPath);
    setInput("");
  };

  return (
    <div className="flex h-full flex-col">
      {/* Project path */}
      <div className="shrink-0 border-b border-border px-3 py-1.5">
        <span className={cn("text-[10px]", projectPath ? "font-mono text-muted-foreground" : "text-xs text-muted-foreground")}>
          {projectPath ?? "Open a folder in Explorer first"}
        </span>
      </div>

      {/* Messages */}
      <div className="flex flex-1 flex-col gap-2 overflow-y-auto p-3">
        {messages.map((msg) => (
          <Message key={msg.id} msg={msg} />
        ))}

        {thinking && (
          <div className="whitespace-pre-wrap break-words rounded-md border border-dashed border-border bg-background px-3 py-2 text-xs text-muted-foreground">
            <span className="mb-1 block text-[10px] font-semibold uppercase tracking-wider text-primary">
              Forge
            </span>
            {thinking}
          </div>
        )}

        {toolCalls.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {toolCalls.map((tc, i) => (
              <span
                key={i}
                className="rounded-sm border border-border bg-accent px-1.5 py-0.5 font-mono text-[10px] text-primary"
              >
                {tc.name}
              </span>
            ))}
          </div>
        )}

        {errors.map((err, i) => (
          <div key={i} className="rounded-md border border-red-800 bg-red-950 px-3 py-2 text-xs text-red-300">
            {err}
          </div>
        ))}

        <div ref={endRef} />
      </div>

      {/* File changes */}
      {files.length > 0 && (
        <div className="flex shrink-0 flex-wrap items-center gap-1 border-t border-border px-3 py-2">
          <span className="text-[10px] text-muted-foreground">Changed:</span>
          {files.map((f, i) => (
            <span
              key={i}
              className="rounded-sm border border-border bg-accent px-1.5 py-0.5 font-mono text-[10px] text-primary"
            >
              {f.action === "delete" ? "-" : f.action === "create" ? "+" : "~"} {f.path}
            </span>
          ))}
        </div>
      )}

      {/* Input */}
      <form className="flex shrink-0 gap-2 border-t border-border p-3" onSubmit={handleSubmit}>
        <input
          type="text"
          className="flex-1 rounded-md border border-input bg-background px-2.5 py-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none"
          placeholder={!projectPath ? "Open a folder first..." : isStreaming ? "Building..." : "Describe what to build..."}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          disabled={isStreaming || !projectPath}
        />
        <Button type="submit" size="sm" disabled={isStreaming || !input.trim() || !projectPath}>
          Build
        </Button>
        {isStreaming && (
          <Button size="sm" variant="destructive" onClick={stopBuild}>
            Stop
          </Button>
        )}
      </form>
    </div>
  );
}
