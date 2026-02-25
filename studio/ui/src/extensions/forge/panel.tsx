import { useState, useRef, useEffect, useSyncExternalStore } from "react";
import {
  subscribe,
  getSnapshot,
  sendMessage,
  abortSession,
  createSession,
  selectSession,
  replyPermission,
  replyQuestion,
  rejectQuestion,
  startForProject,
  type QuestionRequest,
  type QuestionInfo,
} from "./store";
import { Button } from "@/components/ui/button";
import { useProjectContext } from "@/components/layout/app-context";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { SendHorizontal, Square } from "lucide-react";
import { cn } from "@/lib/utils";
import Markdown from "react-markdown";
import type { Message, Part, Permission, Session } from "@opencode-ai/sdk";

const toolStatusLabel: Record<string, string> = {
  completed: "done",
  running: "...",
  error: "err",
};

const heading = (p: React.HTMLAttributes<HTMLHeadingElement>) => <h3 className="mt-2 mb-1 text-xs font-semibold text-foreground" {...p} />;
const mdComponents = {
  p: (p: React.HTMLAttributes<HTMLParagraphElement>) => <p className="my-1 first:mt-0 last:mb-0" {...p} />,
  strong: (p: React.HTMLAttributes<HTMLElement>) => <strong className="font-semibold text-foreground" {...p} />,
  ul: (p: React.HTMLAttributes<HTMLUListElement>) => <ul className="my-1 list-disc pl-4" {...p} />,
  ol: (p: React.OlHTMLAttributes<HTMLOListElement>) => <ol className="my-1 list-decimal pl-4" {...p} />,
  li: (p: React.HTMLAttributes<HTMLLIElement>) => <li className="my-0.5" {...p} />,
  h1: heading, h2: heading, h3: heading,
  code: ({ className, children, ...rest }: React.HTMLAttributes<HTMLElement>) =>
    className ? (
      <pre className="my-1 overflow-x-auto rounded border border-border bg-accent px-2 py-1.5 font-mono text-[10px] leading-snug">
        <code {...rest}>{children}</code>
      </pre>
    ) : (
      <code className="rounded bg-accent px-1 py-0.5 font-mono text-[10px]" {...rest}>{children}</code>
    ),
  pre: ({ children }: React.HTMLAttributes<HTMLPreElement>) => <>{children}</>,
  a: (p: React.AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a className="text-primary underline underline-offset-2" target="_blank" rel="noopener noreferrer" {...p} />
  ),
  hr: () => <hr className="my-2 border-border" />,
  blockquote: (p: React.HTMLAttributes<HTMLQuoteElement>) => (
    <blockquote className="my-1 border-l-2 border-border pl-2 text-muted-foreground" {...p} />
  ),
} as Record<string, React.ComponentType>;

function PartView({ part }: { part: Part }) {
  if (part.type === "text" || part.type === "reasoning") {
    return (
      <div className={cn(
        "break-words text-xs leading-relaxed",
        part.type === "reasoning" && "italic text-muted-foreground",
      )}>
        <Markdown components={mdComponents}>{part.text}</Markdown>
      </div>
    );
  }
  if (part.type === "tool") {
    return (
      <div className="flex min-w-0 items-center gap-1.5 rounded-sm border border-border bg-accent px-2 py-1">
        <span className="shrink-0 font-mono text-[10px] text-primary">{part.tool}</span>
        <span className="shrink-0 text-[10px] text-muted-foreground">
          {toolStatusLabel[part.state.status] ?? ""}
        </span>
        {"title" in part.state && part.state.title && (
          <span className="min-w-0 truncate text-[10px] text-muted-foreground">
            {part.state.title}
          </span>
        )}
      </div>
    );
  }
  return null;
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
        "min-w-0 overflow-hidden rounded-md px-3 py-2 text-xs leading-relaxed",
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
            {msg.error.name}: {"message" in msg.error.data ? String(msg.error.data.message) : "Unknown error"}
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
      <div className="mb-2 truncate font-mono text-[10px] text-muted-foreground">
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

function QuestionFieldView({
  info,
  index,
  answers,
  setAnswers,
}: {
  info: QuestionInfo;
  index: number;
  answers: string[][];
  setAnswers: (fn: React.SetStateAction<string[][]>) => void;
}) {
  const [customText, setCustomText] = useState("");
  const selected = answers[index] || [];

  const set = (value: string[]) =>
    setAnswers((prev) => prev.map((a, i) => (i === index ? value : a)));

  const toggle = (label: string) => {
    if (info.multiple) {
      set(selected.includes(label) ? selected.filter((l) => l !== label) : [...selected, label]);
    } else {
      set(selected[0] === label ? [] : [label]);
    }
  };

  const submitCustom = () => {
    if (!customText.trim()) return;
    set([customText.trim()]);
    setCustomText("");
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="text-[11px] font-medium text-foreground">{info.question}</div>
      <div className="flex flex-col gap-1.5">
        {info.options.map((opt) => (
          <button
            key={opt.label}
            className={cn(
              "flex cursor-pointer flex-col gap-0.5 rounded-md border px-3 py-2 text-left transition-colors",
              selected.includes(opt.label)
                ? "border-primary bg-primary/10 text-foreground"
                : "border-border bg-card text-muted-foreground hover:border-muted-foreground/40 hover:text-foreground",
            )}
            onClick={() => toggle(opt.label)}
          >
            <span className="text-xs font-medium">{opt.label}</span>
            {opt.description && (
              <span className="text-[10px] leading-snug text-muted-foreground">{opt.description}</span>
            )}
          </button>
        ))}
      </div>
      {info.custom !== false && (
        <div className="flex items-center gap-1.5">
          <input
            type="text"
            className="flex-1 rounded-md border border-input bg-background px-2.5 py-1.5 text-xs text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none"
            placeholder="Or type a custom answer..."
            value={customText}
            onChange={(e) => setCustomText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), submitCustom())}
          />
          {customText.trim() && (
            <button className="rounded-md p-1 text-muted-foreground hover:text-foreground" onClick={submitCustom}>
              <SendHorizontal className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

const emptyAnswers = (r: QuestionRequest) => r.questions.map((): string[] => []);

function QuestionsPanel({ requests }: { requests: QuestionRequest[] }) {
  const [allAnswers, setAllAnswers] = useState<Record<string, string[][]>>(() =>
    Object.fromEntries(requests.map((r) => [r.id, emptyAnswers(r)])),
  );

  useEffect(() => {
    setAllAnswers((prev) => {
      const next: Record<string, string[][]> = {};
      for (const r of requests) next[r.id] = prev[r.id] ?? emptyAnswers(r);
      return next;
    });
  }, [requests]);

  const get = (r: QuestionRequest) => allAnswers[r.id] ?? emptyAnswers(r);
  const canSubmit = requests.every((r) => get(r).every((a) => a.length > 0));

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border bg-card p-3">
      <div className="flex items-center gap-2 text-[10px] text-muted-foreground">
        <span className="h-1.5 w-1.5 rounded-full bg-primary animate-pulse" />
        <span className="font-medium uppercase tracking-wider">Questions</span>
        <span className="ml-auto opacity-50">{requests.reduce((n, r) => n + r.questions.length, 0)} items</span>
      </div>
      <div className="flex flex-col gap-4">
        {requests.flatMap((req) =>
          req.questions.map((q, qi) => (
            <QuestionFieldView
              key={`${req.id}-${qi}`}
              info={q}
              index={qi}
              answers={get(req)}
              setAnswers={(fn) =>
                setAllAnswers((prev) => ({
                  ...prev,
                  [req.id]: typeof fn === "function" ? fn(get(req)) : fn,
                }))
              }
            />
          )),
        )}
      </div>
      <div className="flex gap-2 border-t border-border pt-2">
        <Button
          size="sm"
          className="h-7 px-4 text-xs"
          disabled={!canSubmit}
          onClick={() => requests.forEach((r) => replyQuestion(r.id, get(r)))}
        >
          Submit All
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="h-7 px-3 text-xs text-muted-foreground"
          onClick={() => requests.forEach((r) => rejectQuestion(r.id))}
        >
          Skip
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
    <div className="flex min-w-0 items-center gap-2">
      <select
        className="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1 font-mono text-[10px] text-foreground focus:border-ring focus:outline-none"
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
  const { connected, sessionId, sessions, messages, parts, permissions, questions, streaming, error } =
    useSyncExternalStore(subscribe, getSnapshot);
  const { projectPath } = useProjectContext();
  const [input, setInput] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (projectPath) {
      startForProject(projectPath);
    }
  }, [projectPath]);

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, parts, questions, permissions]);

  const submit = () => {
    if (!input.trim() || streaming) return;
    sendMessage(input.trim());
    setInput("");
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
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
          {!connected && (
            <button
              className="ml-1 text-[10px] text-primary hover:underline"
              onClick={() => showAISetupDialog()}
            >
              Configure AI
            </button>
          )}
        </div>
        <SessionSelector
          sessions={sessions}
          currentId={sessionId}
          onSelect={selectSession}
          onCreate={createSession}
        />
      </div>

      <div className="min-h-0 min-w-0 flex-1 space-y-2 overflow-y-auto overflow-x-hidden p-3">
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

        {questions.length > 0 && (
          <QuestionsPanel requests={questions} />
        )}

        {error && (
          <div className="rounded-md border border-red-800 bg-red-950 px-3 py-2 text-xs text-red-300">
            {error}
          </div>
        )}

        <div ref={endRef} />
      </div>

      <div className="shrink-0 border-t border-border p-3">
        <div className="flex flex-col rounded-md border border-input bg-background focus-within:border-ring">
          <textarea
            rows={3}
            className="min-w-0 w-full resize-none border-none bg-transparent px-2.5 pt-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground focus:outline-none"
            placeholder={
              !connected
                ? "Connecting..."
                : streaming
                  ? "Thinking..."
                  : "Ask AI Forge... (Shift+Enter for new line)"
            }
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={streaming || !connected}
          />
          <div className="flex items-center justify-end px-2 py-1">
            {streaming ? (
              <button
                className="rounded-md p-1 text-destructive hover:bg-destructive/10"
                onClick={abortSession}
              >
                <Square className="h-4 w-4" />
              </button>
            ) : (
              <button
                className="rounded-md p-1 text-muted-foreground hover:text-foreground disabled:opacity-30"
                disabled={!input.trim() || !connected}
                onClick={submit}
              >
                <SendHorizontal className="h-4 w-4" />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
