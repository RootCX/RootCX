import { useState, useRef, useEffect, useSyncExternalStore } from "react";
import { subscribe, getSnapshot, sendAgentMessage, abortAgent } from "./store";
import { SendHorizontal, Square } from "lucide-react";
import { cn } from "@/lib/utils";

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

export default function AgentChatPanel({ appId }: { appId: string }) {
  const { agents, chats } = useSyncExternalStore(subscribe, getSnapshot);
  const agent = agents.find((a) => a.app_id === appId);
  const chat = chats[appId];
  const messages = chat?.messages ?? [];
  const streaming = chat?.streaming ?? false;
  const streamed = chat?.streamedText ?? "";
  const error = chat?.error ?? null;

  const [input, setInput] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => { endRef.current?.scrollIntoView({ behavior: "smooth" }); }, [messages, streamed, error]);

  const submit = () => {
    const text = input.trim();
    if (!text || streaming) return;
    sendAgentMessage(appId, text);
    setInput("");
  };

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="shrink-0 border-b border-border px-3 py-1.5">
        <div className="text-xs font-semibold">{agent?.name ?? appId}</div>
        {agent?.description && <div className="text-[10px] text-muted-foreground">{agent.description}</div>}
      </div>

      <div className="min-h-0 min-w-0 flex-1 space-y-2 overflow-y-auto overflow-x-hidden p-3">
        {messages.map((msg, i) => <Bubble key={i} role={msg.role}>{msg.content}</Bubble>)}

        {streaming && streamed && (
          <Bubble role="assistant">
            {streamed}<span className="ml-0.5 inline-block h-3 w-1 animate-pulse bg-foreground" />
          </Bubble>
        )}
        {streaming && !streamed && <div className="text-[10px] text-muted-foreground animate-pulse">Thinking...</div>}
        {error && <div className="rounded-md border border-red-800 bg-red-950 px-3 py-2 text-xs text-red-300">{error}</div>}
        <div ref={endRef} />
      </div>

      <div className="shrink-0 border-t border-border p-3">
        <div className="flex flex-col rounded-md border border-input bg-background focus-within:border-ring">
          <textarea
            rows={3}
            className="min-w-0 w-full resize-none border-none bg-transparent px-2.5 pt-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground focus:outline-none"
            placeholder={streaming ? "Thinking..." : "Type a message... (Shift+Enter for new line)"}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
            disabled={streaming}
          />
          <div className="flex items-center justify-end px-2 py-1">
            {streaming ? (
              <button className="rounded-md p-1 text-destructive hover:bg-destructive/10" onClick={() => abortAgent(appId)}>
                <Square className="h-4 w-4" />
              </button>
            ) : (
              <button className="rounded-md p-1 text-muted-foreground hover:text-foreground disabled:opacity-30" disabled={!input.trim()} onClick={submit}>
                <SendHorizontal className="h-4 w-4" />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
