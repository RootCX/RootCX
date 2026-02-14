import { useState, useCallback, useEffect, useRef } from "react";

const FORGE_BASE = "http://127.0.0.1:3100";

// ── Types ──────────────────────────────────────────────────────────

export type ForgePhase =
  | "idle"
  | "analyzing"
  | "planning"
  | "executing"
  | "verifying"
  | "done"
  | "error"
  | "stopped";

export interface ForgeToolCall {
  name: string;
  args: Record<string, unknown>;
}

export interface ForgeFileChange {
  path: string;
  action: "create" | "update" | "delete";
}

export interface ForgeMessage {
  id: string;
  role: "user" | "assistant" | "status";
  content: string;
  timestamp: number;
}

// ── Hook ───────────────────────────────────────────────────────────

export function useForge(projectId: string) {
  const [messages, setMessages] = useState<ForgeMessage[]>([]);
  const [phase, setPhase] = useState<ForgePhase>("idle");
  const [thinking, setThinking] = useState("");
  const [toolCalls, setToolCalls] = useState<ForgeToolCall[]>([]);
  const [files, setFiles] = useState<ForgeFileChange[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  const [isStreaming, setIsStreaming] = useState(false);
  const [conversationId, setConversationId] = useState<string | null>(null);

  const eventSourceRef = useRef<EventSource | null>(null);
  const thinkingBuffer = useRef("");

  // ── SSE connection ─────────────────────────────────────────────

  const connectSSE = useCallback(() => {
    // Close existing connection
    if (eventSourceRef.current) {
      eventSourceRef.current.close();
    }

    const es = new EventSource(`${FORGE_BASE}/stream/${projectId}`);
    eventSourceRef.current = es;

    es.addEventListener("phase", (e) => {
      const data = JSON.parse(e.data);
      setPhase(data.phase);
    });

    es.addEventListener("agent_thinking", (e) => {
      const data = JSON.parse(e.data);
      thinkingBuffer.current += data.content;
      setThinking(thinkingBuffer.current);
    });

    es.addEventListener("tool_calls", (e) => {
      const data = JSON.parse(e.data);
      setToolCalls(data.calls || []);
    });

    es.addEventListener("tool_executing", (e) => {
      const data = JSON.parse(e.data);
      setMessages((prev) => [
        ...prev,
        {
          id: `tool-${Date.now()}`,
          role: "status",
          content: `Running ${data.name}...`,
          timestamp: Date.now(),
        },
      ]);
    });

    es.addEventListener("tool_result", (e) => {
      const data = JSON.parse(e.data);
      setMessages((prev) => [
        ...prev,
        {
          id: `result-${Date.now()}`,
          role: "status",
          content: `${data.name}: ${data.output}`,
          timestamp: Date.now(),
        },
      ]);
    });

    es.addEventListener("status", (e) => {
      const data = JSON.parse(e.data);
      setMessages((prev) => [
        ...prev,
        {
          id: `status-${Date.now()}`,
          role: "status",
          content: data.message,
          timestamp: Date.now(),
        },
      ]);
    });

    es.addEventListener("error", (e) => {
      const data = JSON.parse(e.data);
      setErrors((prev) => [...prev, data.message]);
    });

    es.addEventListener("complete", (e) => {
      const data = JSON.parse(e.data);
      setIsStreaming(false);
      setPhase(data.success ? "done" : "error");
      setFiles(data.applied_changes || []);

      // Flush thinking buffer as assistant message
      if (thinkingBuffer.current) {
        setMessages((prev) => [
          ...prev,
          {
            id: `assistant-${Date.now()}`,
            role: "assistant",
            content: thinkingBuffer.current,
            timestamp: Date.now(),
          },
        ]);
        thinkingBuffer.current = "";
        setThinking("");
      }

      es.close();
      eventSourceRef.current = null;
    });

    es.onerror = () => {
      // EventSource will auto-reconnect, but we mark not-streaming
      // if the connection fails during idle
      if (phase === "idle") {
        es.close();
        eventSourceRef.current = null;
      }
    };
  }, [projectId, phase]);

  // ── Send message ───────────────────────────────────────────────

  const sendMessage = useCallback(
    async (prompt: string, projectPath: string, appId?: string) => {
      setIsStreaming(true);
      setPhase("analyzing");
      setThinking("");
      setToolCalls([]);
      setErrors([]);
      thinkingBuffer.current = "";

      // Add user message
      setMessages((prev) => [
        ...prev,
        {
          id: `user-${Date.now()}`,
          role: "user",
          content: prompt,
          timestamp: Date.now(),
        },
      ]);

      // Connect SSE first
      connectSSE();

      // Start the build
      try {
        const resp = await fetch(`${FORGE_BASE}/chat`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            project_id: projectId,
            project_path: projectPath,
            prompt,
            conversation_id: conversationId,
            app_id: appId || "",
          }),
        });

        if (!resp.ok) {
          throw new Error(`Forge API error: ${resp.status}`);
        }

        const data = await resp.json();
        setConversationId(data.conversation_id);
      } catch (err) {
        setIsStreaming(false);
        setPhase("error");
        setErrors((prev) => [
          ...prev,
          err instanceof Error ? err.message : String(err),
        ]);
      }
    },
    [projectId, conversationId, connectSSE]
  );

  // ── Stop build ─────────────────────────────────────────────────

  const stopBuild = useCallback(async () => {
    try {
      await fetch(`${FORGE_BASE}/stop`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ project_id: projectId }),
      });
    } catch {
      // Best-effort
    }
    setIsStreaming(false);
    setPhase("stopped");
  }, [projectId]);

  // ── Cleanup ────────────────────────────────────────────────────

  useEffect(() => {
    return () => {
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
      }
    };
  }, []);

  return {
    messages,
    phase,
    thinking,
    toolCalls,
    files,
    errors,
    isStreaming,
    conversationId,
    sendMessage,
    stopBuild,
  };
}
