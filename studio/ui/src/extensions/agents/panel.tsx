import { useState, useRef, useEffect, useSyncExternalStore } from "react";
import { subscribe, getSnapshot, sendAgentMessage, abortAgent, approveToolCall, rejectToolCall } from "./store";
import { SendHorizontal, Square, Unplug, Check, X, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import type { AgentMessage } from "@/types";

const bubbleBase = "min-w-0 overflow-hidden rounded-md px-3 py-2 text-xs leading-relaxed whitespace-pre-wrap break-words";

function Bubble({ role, children }: { role: "user" | "assistant"; children: React.ReactNode }) {
  return (
    <div className={cn(bubbleBase,
      role === "user" ? "self-end max-w-[80%] border border-blue-600 bg-blue-950" : "border border-border bg-background",
    )}>
      <span className="mb-1 block text-[10px] font-semibold uppercase tracking-wider text-primary">
        {role === "user" ? "You" : "Assistant"}
      </span>
      {children}
    </div>
  );
}

function ApprovalCard({ msg, appId, pending }: { msg: AgentMessage; appId: string; pending: boolean }) {
  const approvalId = msg.meta?.approvalId as string;
  const toolName = msg.meta?.toolName as string;
  return (
    <div className="rounded-md border border-amber-700 bg-amber-950/50 px-3 py-2 text-xs">
      <div className="mb-1 text-[10px] font-semibold uppercase tracking-wider text-amber-400">Approval Required</div>
      <div className="mb-1 text-muted-foreground">{msg.content}</div>
      <div className="mb-2 font-mono text-[10px] text-amber-300">{toolName}</div>
      {pending && (
        <div className="flex gap-1.5">
          <Button size="xs" variant="outline" className="gap-1 border-green-700 text-green-400 hover:bg-green-950"
            onClick={() => approveToolCall(appId, approvalId)}>
            <Check className="h-3 w-3" /> Approve
          </Button>
          <Button size="xs" variant="outline" className="gap-1 border-red-700 text-red-400 hover:bg-red-950"
            onClick={() => rejectToolCall(appId, approvalId)}>
            <X className="h-3 w-3" /> Reject
          </Button>
        </div>
      )}
    </div>
  );
}

function ToolActivity({ msg }: { msg: AgentMessage }) {
  const isStart = msg.type === "tool_start";
  return (
    <div className="flex items-center gap-1.5 text-[10px] text-muted-foreground">
      {isStart && <Loader2 className="h-3 w-3 animate-spin" />}
      <span className="font-mono">{msg.content}</span>
    </div>
  );
}

function MessageItem({ msg, appId, isPendingApproval }: { msg: AgentMessage; appId: string; isPendingApproval: boolean }) {
  if (msg.type === "approval") return <ApprovalCard msg={msg} appId={appId} pending={isPendingApproval} />;
  if (msg.type === "tool_start" || msg.type === "tool_done") return <ToolActivity msg={msg} />;
  return <Bubble role={msg.role as "user" | "assistant"}>{msg.content}</Bubble>;
}

export default function AgentChatPanel({ appId, name }: { appId: string; name?: string }) {
  const { chats, deployed } = useSyncExternalStore(subscribe, getSnapshot);
  const isDeployed = deployed[appId] === true;
  const chat = chats[appId];
  const messages = chat?.messages ?? [];
  const streaming = chat?.streaming ?? false;
  const streamed = chat?.streamedText ?? "";
  const error = chat?.error ?? null;
  const pendingApprovals = chat?.pendingApprovals ?? {};
  const disabled = !isDeployed || streaming;

  const [input, setInput] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => { endRef.current?.scrollIntoView({ behavior: "smooth" }); }, [messages, streamed, error]);

  const submit = () => {
    const text = input.trim();
    if (!text || disabled) return;
    sendAgentMessage(appId, text);
    setInput("");
  };

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="shrink-0 border-b border-border px-3 py-1.5">
        <div className="truncate text-xs font-semibold">{name ?? appId}</div>
      </div>

      {!isDeployed && messages.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <Unplug className="h-16 w-16 text-muted-foreground/20" strokeWidth={1} />
        </div>
      ) : (
        <div className="min-h-0 min-w-0 flex-1 space-y-2 overflow-y-auto overflow-x-hidden p-3">
          {messages.map((msg, i) => (
            <MessageItem
              key={`${msg.role}-${msg.type ?? ""}-${i}`}
              msg={msg}
              appId={appId}
              isPendingApproval={msg.type === "approval" && !!(msg.meta?.approvalId as string) && (msg.meta?.approvalId as string) in pendingApprovals}
            />
          ))}

          {streaming && streamed && (
            <Bubble role="assistant">
              {streamed}<span className="ml-0.5 inline-block h-3 w-1 animate-pulse bg-foreground" />
            </Bubble>
          )}
          {streaming && !streamed && <div className="text-[10px] text-muted-foreground animate-pulse">Thinking...</div>}
          {error && <div className="rounded-md border border-red-800 bg-red-950 px-3 py-2 text-xs text-red-300">{error}</div>}
          <div ref={endRef} />
        </div>
      )}

      <div className="shrink-0 border-t border-border p-3">
        <div className={cn("flex flex-col rounded-md border border-input bg-background focus-within:border-ring", !isDeployed && "opacity-40")}>
          <textarea
            rows={3}
            className="min-w-0 w-full resize-none border-none bg-transparent px-2.5 pt-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground focus:outline-none"
            placeholder={!isDeployed ? "Deploy the agent to start chatting..." : streaming ? "Thinking..." : "Type a message... (Shift+Enter for new line)"}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
            disabled={disabled}
          />
          <div className="flex items-center justify-end px-2 py-1">
            {streaming ? (
              <Button size="icon-xs" variant="ghost" className="text-destructive hover:bg-destructive/10" onClick={() => abortAgent(appId)}>
                <Square className="h-4 w-4" />
              </Button>
            ) : (
              <Button size="icon-xs" variant="ghost" disabled={!input.trim() || !isDeployed} onClick={submit}>
                <SendHorizontal className="h-4 w-4" />
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
