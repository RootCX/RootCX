import { useState, useRef, useEffect, useSyncExternalStore } from "react";
import {
  subscribe, getSnapshot, sendMessage, abortSession,
  createSession, selectSession, replyPermission,
  replyQuestion, rejectQuestion, startForProject,
  type QuestionRequest, type QuestionInfo,
} from "./store";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { useProjectContext } from "@/components/layout/app-context";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { ArrowUp, Square, Plus, ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";
import Markdown from "react-markdown";
import type { Message, Part, Permission, Session } from "@opencode-ai/sdk";

const heading = (p: React.HTMLAttributes<HTMLHeadingElement>) => (
  <h3 className="mt-4 mb-2 text-[15px] font-semibold text-foreground" {...p} />
);

const mdComponents = {
  p: (p: React.HTMLAttributes<HTMLParagraphElement>) => <p className="my-2 first:mt-0 last:mb-0 leading-relaxed" {...p} />,
  strong: (p: React.HTMLAttributes<HTMLElement>) => <strong className="font-semibold text-foreground" {...p} />,
  ul: (p: React.HTMLAttributes<HTMLUListElement>) => <ul className="my-2 list-disc pl-5 marker:text-muted-foreground/50" {...p} />,
  ol: (p: React.OlHTMLAttributes<HTMLOListElement>) => <ol className="my-2 list-decimal pl-5" {...p} />,
  li: (p: React.HTMLAttributes<HTMLLIElement>) => <li className="my-1 leading-relaxed" {...p} />,
  h1: heading, h2: heading, h3: heading,
  code: ({ className, children, ...rest }: React.HTMLAttributes<HTMLElement>) =>
    className
      ? <pre className="my-3 overflow-x-auto rounded-lg bg-[#141414] px-4 py-3 font-mono text-[13px] leading-relaxed text-foreground/90"><code {...rest}>{children}</code></pre>
      : <code className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[13px] text-foreground/90" {...rest}>{children}</code>,
  pre: ({ children }: React.HTMLAttributes<HTMLPreElement>) => <>{children}</>,
  a: (p: React.AnchorHTMLAttributes<HTMLAnchorElement>) => <a className="text-primary hover:text-primary/80 underline underline-offset-2 transition-colors" target="_blank" rel="noopener noreferrer" {...p} />,
  hr: () => <hr className="my-4 border-border/50" />,
  blockquote: (p: React.HTMLAttributes<HTMLQuoteElement>) => <blockquote className="my-2 border-l-2 border-primary/30 pl-4 text-muted-foreground italic" {...p} />,
} as Record<string, React.ComponentType>;

const TOOL_STATUS: Record<string, string> = { completed: "done", running: "running", error: "failed" };
const NO_PARTS: Part[] = [];

function PartView({ part }: { part: Part }) {
  if (part.type === "text" || part.type === "reasoning") {
    return (
      <div className={cn("break-words text-[14px] leading-[1.7]", part.type === "reasoning" && "italic text-muted-foreground/80")}>
        <Markdown components={mdComponents}>{part.text}</Markdown>
      </div>
    );
  }
  if (part.type === "tool") {
    return (
      <div className="flex min-w-0 items-center gap-2 py-0.5 text-xs text-muted-foreground">
        <span className={cn(
          "h-1.5 w-1.5 shrink-0 rounded-full",
          part.state.status === "running" ? "bg-primary animate-[pulse-dot_1.5s_infinite]"
            : part.state.status === "error" ? "bg-destructive" : "bg-green-500",
        )} />
        <span className="shrink-0 font-mono text-foreground/70">{part.tool}</span>
        <span className="shrink-0 text-muted-foreground/50">{TOOL_STATUS[part.state.status] ?? ""}</span>
        {"title" in part.state && part.state.title && (
          <span className="min-w-0 truncate text-muted-foreground/40">{part.state.title}</span>
        )}
      </div>
    );
  }
  return null;
}

function MessageView({ msg, parts }: { msg: Message; parts: Part[] }) {
  const isUser = msg.role === "user";
  return (
    <div className={cn("flex min-w-0", isUser ? "justify-end" : "justify-start")}>
      <div className={cn(
        "min-w-0 overflow-hidden text-[14px] leading-[1.7]",
        isUser && "max-w-[80%] rounded-2xl rounded-br-md bg-muted/80 px-4 py-2.5",
        !isUser && "w-full",
      )}>
        {parts.length > 0 ? (
          <div className={cn("flex flex-col", isUser ? "gap-1" : "gap-0.5")}>
            {parts.map((p) => <PartView key={p.id} part={p} />)}
          </div>
        ) : (
          msg.role === "assistant" && msg.error && (
            <span className="text-sm text-destructive/80">
              {msg.error.name}: {"message" in msg.error.data ? String(msg.error.data.message) : "Unknown error"}
            </span>
          )
        )}
      </div>
    </div>
  );
}

const PERM_BTN = "h-8 rounded-lg text-xs";

function PermissionCard({ perm }: { perm: Permission }) {
  return (
    <div className="rounded-xl border border-yellow-500/20 bg-yellow-500/5 px-5 py-4">
      <div className="mb-2 text-sm font-medium text-yellow-200/90">{perm.title}</div>
      <div className="mb-3 truncate font-mono text-xs text-muted-foreground/60">
        {perm.type}
        {perm.pattern && `: ${Array.isArray(perm.pattern) ? perm.pattern.join(", ") : perm.pattern}`}
      </div>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" className={cn(PERM_BTN, "border-yellow-500/20 hover:bg-yellow-500/10")} onClick={() => replyPermission(perm.id, "once")}>Allow Once</Button>
        <Button size="sm" variant="outline" className={cn(PERM_BTN, "border-yellow-500/20 hover:bg-yellow-500/10")} onClick={() => replyPermission(perm.id, "always")}>Always Allow</Button>
        <Button size="sm" variant="ghost" className={cn(PERM_BTN, "text-muted-foreground hover:text-foreground")} onClick={() => replyPermission(perm.id, "reject")}>Deny</Button>
      </div>
    </div>
  );
}

function QuestionFieldView({
  info, index, answers, setAnswers,
}: {
  info: QuestionInfo; index: number; answers: string[][];
  setAnswers: (fn: React.SetStateAction<string[][]>) => void;
}) {
  const [customText, setCustomText] = useState("");
  const selected = answers[index] || [];
  const set = (value: string[]) => setAnswers((prev) => prev.map((a, i) => (i === index ? value : a)));

  const toggle = (label: string) => {
    if (info.multiple) set(selected.includes(label) ? selected.filter((l) => l !== label) : [...selected, label]);
    else set(selected[0] === label ? [] : [label]);
  };

  return (
    <div className="flex flex-col gap-2.5">
      <div className="text-sm font-medium text-foreground">{info.question}</div>
      <div className="flex flex-col gap-2">
        {info.options.map((opt) => (
          <button
            key={opt.label}
            className={cn(
              "flex cursor-pointer flex-col gap-1 rounded-xl border px-4 py-3 text-left transition-all",
              selected.includes(opt.label)
                ? "border-primary/40 bg-primary/5 text-foreground"
                : "border-border/50 text-muted-foreground hover:border-border hover:text-foreground",
            )}
            onClick={() => toggle(opt.label)}
          >
            <span className="text-sm font-medium">{opt.label}</span>
            {opt.description && <span className="text-xs leading-relaxed text-muted-foreground/70">{opt.description}</span>}
          </button>
        ))}
      </div>
      {info.custom !== false && (
        <input
          type="text"
          className="rounded-xl border border-border/50 bg-transparent px-4 py-2.5 text-sm text-foreground placeholder:text-muted-foreground/40 focus:border-border focus:outline-none"
          placeholder="Or type a custom answer..."
          value={customText}
          onChange={(e) => setCustomText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || !customText.trim()) return;
            e.preventDefault();
            set([customText.trim()]);
            setCustomText("");
          }}
        />
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
    <div className="flex flex-col gap-5 rounded-2xl border border-border/50 bg-card/50 p-5">
      {requests.flatMap((req) =>
        req.questions.map((q, qi) => (
          <QuestionFieldView
            key={`${req.id}-${qi}`}
            info={q} index={qi} answers={get(req)}
            setAnswers={(fn) => setAllAnswers((prev) => ({
              ...prev, [req.id]: typeof fn === "function" ? fn(get(req)) : fn,
            }))}
          />
        )),
      )}
      <div className="flex gap-2 border-t border-border/30 pt-4">
        <Button size="sm" className="h-9 rounded-xl px-5 text-sm" disabled={!canSubmit} onClick={() => requests.forEach((r) => replyQuestion(r.id, get(r)))}>Submit</Button>
        <Button size="sm" variant="ghost" className="h-9 rounded-xl px-4 text-sm text-muted-foreground" onClick={() => requests.forEach((r) => rejectQuestion(r.id))}>Skip</Button>
      </div>
    </div>
  );
}

function SessionSelector({ sessions, currentId, onSelect, onCreate }: {
  sessions: Session[]; currentId: string | null; onSelect: (id: string) => void; onCreate: () => void;
}) {
  return (
    <div className="flex items-center gap-1.5">
      {sessions.length > 0 && (
        <div className="relative">
          <select
            className="h-7 cursor-pointer appearance-none rounded-lg bg-transparent py-0 pl-2.5 pr-7 text-xs text-muted-foreground transition-colors hover:text-foreground focus:outline-none"
            value={currentId ?? ""}
            onChange={(e) => { if (e.target.value) onSelect(e.target.value); }}
          >
            {sessions.map((s) => <option key={s.id} value={s.id}>{s.title || s.id.slice(0, 8)}</option>)}
          </select>
          <ChevronDown className="pointer-events-none absolute right-1.5 top-1/2 h-3 w-3 -translate-y-1/2 text-muted-foreground/50" />
        </div>
      )}
      <button className="flex h-7 w-7 items-center justify-center rounded-lg text-muted-foreground/50 transition-colors hover:bg-muted hover:text-foreground" onClick={onCreate} title="New session">
        <Plus className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

function Composer({ input, setInput, onSubmit, onAbort, connected, streaming, className }: {
  input: string; setInput: (v: string) => void; onSubmit: () => void; onAbort: () => void;
  connected: boolean; streaming: boolean; className?: string;
}) {
  return (
    <div className={cn("mx-auto w-full max-w-3xl", className)}>
      <div className="flex flex-col rounded-2xl border border-border/60 bg-card shadow-lg shadow-black/20 transition-colors focus-within:border-border">
        <textarea
          rows={3}
          className="w-full resize-none bg-transparent px-5 pt-4 pb-1 text-[14px] leading-relaxed text-foreground placeholder:text-muted-foreground/40 focus:outline-none"
          placeholder={!connected ? "Connecting..." : streaming ? "Thinking..." : "What do you want to build?"}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); onSubmit(); } }}
          disabled={streaming || !connected}
        />
        <div className="flex items-center justify-end px-3 pb-3">
          {!connected && (
            <button className="mr-auto text-xs text-primary/70 transition-colors hover:text-primary" onClick={() => showAISetupDialog()}>Configure AI</button>
          )}
          {streaming ? (
            <button className="flex h-8 w-8 items-center justify-center rounded-full bg-muted text-foreground transition-colors hover:bg-muted-foreground/20" onClick={onAbort}>
              <Square className="h-3.5 w-3.5" />
            </button>
          ) : (
            <button
              className={cn("flex h-8 w-8 items-center justify-center rounded-full transition-all",
                input.trim() && connected ? "bg-primary text-primary-foreground hover:bg-primary/90" : "bg-muted text-muted-foreground/30")}
              disabled={!input.trim() || !connected}
              onClick={onSubmit}
            >
              <ArrowUp className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

export default function ForgePanel() {
  const { connected, sessionId, sessions, messages, parts, permissions, questions, streaming, error } =
    useSyncExternalStore(subscribe, getSnapshot);
  const { projectPath } = useProjectContext();
  const [input, setInput] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => { if (projectPath) startForProject(projectPath); }, [projectPath]);
  useEffect(() => { endRef.current?.scrollIntoView({ behavior: "smooth" }); }, [messages, parts, questions, permissions]);

  const submit = () => {
    if (!input.trim() || streaming) return;
    sendMessage(input.trim());
    setInput("");
  };

  const composerProps = { input, setInput, onSubmit: submit, onAbort: abortSession, connected, streaming };
  const sessionProps = { sessions, currentId: sessionId, onSelect: selectSession, onCreate: createSession };
  const hasContent = messages.length + permissions.length + questions.length > 0 || !!error;

  if (!hasContent) {
    return (
      <div className="relative flex h-full flex-col items-center justify-center overflow-hidden px-6">
        <Logo className="pointer-events-none absolute h-[50%] max-h-[320px] text-white/[0.02]" />
        <div className="z-10 flex w-full max-w-3xl flex-col items-center gap-8">
          <div className="flex flex-col items-center gap-3">
            <h1 className="text-2xl font-medium tracking-tight text-foreground/80">What can I help you build?</h1>
            <p className="text-sm text-muted-foreground/50">Describe what you want to create. I can write code, build features, and help you ship.</p>
          </div>
          <Composer {...composerProps} className="w-full" />
        </div>
        <div className="absolute bottom-3 right-3">
          <SessionSelector {...sessionProps} />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center justify-end px-4 py-2">
        <SessionSelector {...sessionProps} />
      </div>
      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto overflow-x-hidden">
        <div className="mx-auto w-full max-w-3xl space-y-5 px-6 py-6">
          {messages.map((msg) => <MessageView key={msg.id} msg={msg} parts={parts.get(msg.id) ?? NO_PARTS} />)}
          {permissions.map((perm) => <PermissionCard key={perm.id} perm={perm} />)}
          {questions.length > 0 && <QuestionsPanel requests={questions} />}
          {error && <div className="rounded-xl border border-destructive/20 bg-destructive/5 px-5 py-3 text-sm text-destructive/80">{error}</div>}
          <div ref={endRef} />
        </div>
      </div>
      <div className="shrink-0 px-6 pb-5 pt-2">
        <Composer {...composerProps} />
      </div>
    </div>
  );
}
