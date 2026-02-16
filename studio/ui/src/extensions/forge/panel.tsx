import { useState, useRef, useEffect, useSyncExternalStore, lazy, Suspense } from "react";
import {
  subscribe,
  getSnapshot,
  sendMessage,
  abortSession,
  createSession,
  selectSession,
  replyPermission,
  startForProject,
} from "./store";
import { Button } from "@/components/ui/button";
import { useProjectContext } from "@/components/layout/app-context";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from "@/components/ui/tooltip";
import { Settings } from "lucide-react";
import { cn } from "@/lib/utils";
import type { Message, Part, Permission, Session } from "@opencode-ai/sdk";

const ForgeSettings = lazy(() => import("./settings"));

function TextPartView({ part }: { part: Extract<Part, { type: "text" }> }) {
  return (
    <div className="whitespace-pre-wrap break-words text-xs leading-relaxed">
      {part.text}
    </div>
  );
}

function ReasoningPartView({
  part,
}: {
  part: Extract<Part, { type: "reasoning" }>;
}) {
  return (
    <div className="whitespace-pre-wrap break-words text-xs italic text-muted-foreground">
      {part.text}
    </div>
  );
}

function ToolPartView({ part }: { part: Extract<Part, { type: "tool" }> }) {
  const statusIcon =
    part.state.status === "completed"
      ? "done"
      : part.state.status === "running"
        ? "..."
        : part.state.status === "error"
          ? "err"
          : "";
  return (
    <div className="flex items-center gap-1.5 rounded-sm border border-border bg-accent px-2 py-1">
      <span className="font-mono text-[10px] text-primary">{part.tool}</span>
      <span className="text-[10px] text-muted-foreground">{statusIcon}</span>
      {"title" in part.state && part.state.title && (
        <span className="text-[10px] text-muted-foreground">
          {part.state.title}
        </span>
      )}
    </div>
  );
}

function PartView({ part }: { part: Part }) {
  switch (part.type) {
    case "text":
      return <TextPartView part={part} />;
    case "reasoning":
      return <ReasoningPartView part={part} />;
    case "tool":
      return <ToolPartView part={part} />;
    default:
      return null;
  }
}

function MessageView({
  msg,
  parts,
}: {
  msg: Message;
  parts: Part[];
}) {
  const isUser = msg.role === "user";
  return (
    <div
      className={cn(
        "rounded-md px-3 py-2 text-xs leading-relaxed",
        isUser && "self-end max-w-[80%] border border-blue-600 bg-blue-950",
        !isUser && "border border-border bg-background",
      )}
    >
      <span className="mb-1 block text-[10px] font-semibold uppercase tracking-wider text-primary">
        {isUser ? "You" : "Assistant"}
      </span>
      {parts.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          {parts.map((part) => (
            <PartView key={part.id} part={part} />
          ))}
        </div>
      ) : (
        msg.role === "assistant" &&
        msg.error && (
          <span className="text-red-400">
            {msg.error.name}: {"message" in msg.error.data ? msg.error.data.message : "Unknown error"}
          </span>
        )
      )}
    </div>
  );
}

function PermissionCard({ perm }: { perm: Permission }) {
  return (
    <div className="rounded-md border border-yellow-700 bg-yellow-950/50 px-3 py-2 text-xs">
      <div className="mb-1 font-semibold text-yellow-300">{perm.title}</div>
      <div className="mb-2 font-mono text-[10px] text-muted-foreground">
        {perm.type}
        {perm.pattern &&
          `: ${Array.isArray(perm.pattern) ? perm.pattern.join(", ") : perm.pattern}`}
      </div>
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          className="h-6 text-[10px]"
          onClick={() => replyPermission(perm.id, "once")}
        >
          Allow Once
        </Button>
        <Button
          size="sm"
          variant="outline"
          className="h-6 text-[10px]"
          onClick={() => replyPermission(perm.id, "always")}
        >
          Always Allow
        </Button>
        <Button
          size="sm"
          variant="destructive"
          className="h-6 text-[10px]"
          onClick={() => replyPermission(perm.id, "reject")}
        >
          Deny
        </Button>
      </div>
    </div>
  );
}

function SessionSelector({
  sessions,
  currentId,
  onSelect,
  onCreate,
}: {
  sessions: Session[];
  currentId: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
}) {
  return (
    <div className="flex items-center gap-2">
      <select
        className="flex-1 rounded-md border border-input bg-background px-2 py-1 font-mono text-[10px] text-foreground focus:border-ring focus:outline-none"
        value={currentId ?? ""}
        onChange={(e) => {
          if (e.target.value) onSelect(e.target.value);
        }}
      >
        <option value="">Select session...</option>
        {sessions.map((s) => (
          <option key={s.id} value={s.id}>
            {s.title || s.id.slice(0, 8)}
          </option>
        ))}
      </select>
      <Button size="sm" variant="outline" className="h-6 text-[10px]" onClick={onCreate}>
        New
      </Button>
    </div>
  );
}

export default function ForgePanel() {
  const { connected, sessionId, sessions, messages, parts, permissions, streaming, error } =
    useSyncExternalStore(subscribe, getSnapshot);
  const { projectPath } = useProjectContext();
  const [input, setInput] = useState("");
  const [view, setView] = useState<"chat" | "settings">("chat");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (projectPath) {
      startForProject(projectPath);
    }
  }, [projectPath]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, parts]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!input.trim() || streaming) return;
    sendMessage(input.trim());
    setInput("");
  };

  return (
    <div className="flex h-full flex-col">
      {/* Header: connection status + session selector + gear icon */}
      <div className="flex shrink-0 flex-col gap-1.5 border-b border-border px-3 py-1.5">
        <div className="flex items-center gap-1.5">
          <span
            className={cn(
              "h-2 w-2 rounded-full",
              connected ? "bg-green-500" : "bg-red-500",
            )}
          />
          <span className="text-[10px] text-muted-foreground">
            {connected ? "Connected" : "Disconnected"}
          </span>
          <div className="ml-auto">
            <TooltipProvider>
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    className={cn(
                      "rounded-sm p-0.5 text-muted-foreground hover:text-foreground",
                      view === "settings" && "text-foreground",
                    )}
                    onClick={() =>
                      setView((v) => (v === "chat" ? "settings" : "chat"))
                    }
                  >
                    <Settings className="h-3.5 w-3.5" />
                  </button>
                </TooltipTrigger>
                <TooltipContent side="left">
                  {view === "settings" ? "Back to chat" : "Settings"}
                </TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </div>
        </div>
        {view === "chat" && (
          <SessionSelector
            sessions={sessions}
            currentId={sessionId}
            onSelect={selectSession}
            onCreate={createSession}
          />
        )}
      </div>

      {view === "settings" ? (
        <div className="flex-1 overflow-y-auto">
          <Suspense
            fallback={
              <div className="p-3 text-[10px] text-muted-foreground">
                Loading...
              </div>
            }
          >
            <ForgeSettings />
          </Suspense>
        </div>
      ) : (
        <>
          {/* Messages */}
          <div className="flex flex-1 flex-col gap-2 overflow-y-auto p-3">
            {messages.map((msg) => (
              <MessageView
                key={msg.id}
                msg={msg}
                parts={parts.get(msg.id) || []}
              />
            ))}

            {permissions.map((perm) => (
              <PermissionCard key={perm.id} perm={perm} />
            ))}

            {error && (
              <div className="rounded-md border border-red-800 bg-red-950 px-3 py-2 text-xs text-red-300">
                {error}
              </div>
            )}

            <div ref={endRef} />
          </div>

          {/* Input */}
          <form
            className="flex shrink-0 gap-2 border-t border-border p-3"
            onSubmit={handleSubmit}
          >
            <input
              type="text"
              className="flex-1 rounded-md border border-input bg-background px-2.5 py-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none"
              placeholder={
                !connected
                  ? "Connecting..."
                  : streaming
                    ? "Thinking..."
                    : "Ask AI Forge..."
              }
              value={input}
              onChange={(e) => setInput(e.target.value)}
              disabled={streaming || !connected}
            />
            <Button
              type="submit"
              size="sm"
              disabled={streaming || !input.trim() || !connected}
            >
              Send
            </Button>
            {streaming && (
              <Button size="sm" variant="destructive" onClick={abortSession}>
                Stop
              </Button>
            )}
          </form>
        </>
      )}
    </div>
  );
}
