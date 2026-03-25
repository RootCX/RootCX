import { useState, useEffect, useSyncExternalStore } from "react";
import {
  subscribe, getSnapshot, sendMessage, abortSession,
  createSession, selectSession, replyPermission,
  replyQuestion, rejectQuestion, setCwd,
  type QuestionRequest, type QuestionInfo,
  type Part, type Permission, type Session,
} from "./store";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { useProjectContext } from "@/components/layout/app-context";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { llmStore } from "@/lib/ai-models";
import { ArrowUp, Square, Plus, ChevronDown, ChevronRight, Check, X, Loader2, Terminal, Search, FileText, FolderOpen, Pencil, Globe, Eye } from "lucide-react";
import { cn } from "@/lib/utils";
import { Markdown, ChatScrollArea } from "@rootcx/ui";

const NO_PARTS: Part[] = [];

const TOOL_META: Record<string, { label: string; icon: typeof FileText }> = {
  read: { label: "Read", icon: Eye },
  write: { label: "Write", icon: FileText },
  edit: { label: "Edit", icon: Pencil },
  bash: { label: "Run", icon: Terminal },
  grep: { label: "Search", icon: Search },
  glob: { label: "Find files", icon: FolderOpen },
  ls: { label: "Browse", icon: FolderOpen },
  web_fetch: { label: "Fetch", icon: Globe },
};

function toolSummary(part: Part): { label: string; target: string | null } {
  const meta = TOOL_META[part.tool_name ?? ""];
  const label = part.tool_state?.title ?? meta?.label ?? part.tool_name ?? "Tool";
  const inp = part.tool_input ?? {};
  const target = (inp.command as string)
    ? `$ ${inp.command}`
    : (inp.pattern as string)
    ? `/${inp.pattern}/`
    : (inp.file_path as string) ?? (inp.url as string) ?? (inp.path as string) ?? null;
  return { label, target };
}

function StatusIcon({ status }: { status: string }) {
  if (status === "streaming" || status === "running") return <Loader2 className="h-3 w-3 animate-spin text-primary" />;
  if (status === "error") return <X className="h-3 w-3 text-destructive" />;
  return <Check className="h-3 w-3 text-muted-foreground/50" />;
}

function runningPreview(part: Part): string | null {
  const inp = part.tool_input ?? {};
  const name = part.tool_name;
  if (name === "write") return (inp.content as string) ?? null;
  if (name === "edit") {
    const ns = inp.new_string as string;
    if (ns) return ns;
    const os = inp.old_string as string;
    return os ? `- ${os}` : null;
  }
  if (name === "bash") return (inp.command as string) ? `$ ${inp.command}` : null;
  return null;
}

function ToolCard({ part }: { part: Part }) {
  const [expanded, setExpanded] = useState(false);
  const status = part.tool_state?.status ?? "completed";
  const { label, target } = toolSummary(part);
  const Icon = TOOL_META[part.tool_name ?? ""]?.icon ?? Terminal;
  const active = status === "running" || status === "streaming";
  const isStreaming = status === "streaming";

  const previewText = active ? (part.content || runningPreview(part)) : part.content || null;
  const lines = previewText ? previewText.split("\n") : [];
  const hasPreview = lines.length > 0;
  // Streaming: tail (see code being written). Completed: head (summary).
  const visibleLines = isStreaming ? lines.slice(-6) : lines.slice(0, 4);
  const truncAbove = isStreaming && lines.length > 6;
  const truncBelow = !isStreaming && lines.length > 4;
  const previewCls = cn("ml-[26px] mr-2 mt-0.5 rounded-md bg-muted/30 px-2.5 py-1.5", active && "border-l-2 border-primary/30");

  return (
    <div className="group animate-[settle_0.35s_cubic-bezier(0.16,1,0.3,1)_both]">
      <button
        onClick={() => hasPreview && setExpanded((e) => !e)}
        className={cn(
          "flex w-full items-center gap-2 rounded-lg px-2 py-1 text-left text-xs transition-colors",
          hasPreview ? "cursor-pointer hover:bg-muted/50" : "cursor-default",
        )}
      >
        <StatusIcon status={status} />
        <Icon className="h-3 w-3 shrink-0 text-muted-foreground/40" />
        <span className="font-medium text-muted-foreground/70">{label}</span>
        {target && <span className="min-w-0 truncate font-mono text-muted-foreground/40">{target}</span>}
        {(truncBelow || (hasPreview && !isStreaming)) && (
          <ChevronRight className={cn("ml-auto h-3 w-3 shrink-0 text-muted-foreground/30 transition-transform duration-150", expanded && "rotate-90")} />
        )}
      </button>
      {hasPreview && !expanded && (
        <div className={cn(previewCls, "overflow-hidden")}>
          <pre className="text-[11px] leading-relaxed text-muted-foreground/50 font-mono whitespace-pre-wrap break-all">
            {truncAbove ? "⋯\n" : ""}{visibleLines.join("\n")}{truncBelow ? "\n⋯" : ""}
          </pre>
        </div>
      )}
      {expanded && (
        <div className={cn(previewCls, "max-h-[400px] overflow-y-auto")}>
          <pre className="text-[11px] leading-relaxed text-muted-foreground/50 font-mono whitespace-pre-wrap break-all">{previewText}</pre>
        </div>
      )}
    </div>
  );
}

function ThinkingBlock({ part, isStreaming }: { part: Part; isStreaming: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const lines = part.content.split("\n").filter(Boolean);
  const running = isStreaming && !part.content.endsWith("\n\n");
  // Show last 2 lines as teaser while streaming
  const teaser = running ? lines.slice(-2).join(" ") : lines[0] ?? "";

  return (
    <div className="animate-[settle_0.35s_cubic-bezier(0.16,1,0.3,1)_both]">
      <button
        onClick={() => setExpanded((e) => !e)}
        className="flex items-center gap-2 rounded-lg px-2 py-1 text-xs text-muted-foreground/50 transition-colors hover:bg-muted/50"
      >
        {running ? <Loader2 className="h-3 w-3 animate-spin" /> : <ChevronRight className={cn("h-3 w-3 transition-transform duration-150", expanded && "rotate-90")} />}
        <span className="font-medium">Thinking</span>
        {!expanded && teaser && <span className="min-w-0 truncate italic opacity-60">{teaser}</span>}
      </button>
      {expanded && (
        <div className="ml-[26px] mr-2 mt-1 max-h-[300px] overflow-y-auto">
          <Markdown className="text-xs italic text-muted-foreground/50 leading-relaxed">{part.content}</Markdown>
        </div>
      )}
    </div>
  );
}

function MessageBubble({ parts }: { parts: Part[] }) {
  return (
    <div className="flex justify-end">
      <div className="max-w-[80%] rounded-2xl rounded-br-md bg-muted/80 px-4 py-2.5 text-[14px] leading-[1.7]">
        {parts.map((p) => <Markdown key={p.id}>{p.content}</Markdown>)}
      </div>
    </div>
  );
}

const PERM_BTN = "h-8 rounded-lg text-xs";

function PermissionCard({ perm }: { perm: Permission }) {
  return (
    <div className="rounded-xl border border-yellow-500/20 bg-yellow-500/5 px-5 py-4">
      <div className="mb-2 text-sm font-medium text-yellow-200/90">{perm.title}</div>
      <div className="mb-3 truncate font-mono text-xs text-muted-foreground/60">{perm.description}</div>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" className={cn(PERM_BTN, "border-yellow-500/20 hover:bg-yellow-500/10")} onClick={() => replyPermission(perm.id, "once")}>Allow Once</Button>
        <Button size="sm" variant="outline" className={cn(PERM_BTN, "border-yellow-500/20 hover:bg-yellow-500/10")} onClick={() => replyPermission(perm.id, "always")}>Always Allow</Button>
        <Button size="sm" variant="ghost" className={cn(PERM_BTN, "text-muted-foreground hover:text-foreground")} onClick={() => replyPermission(perm.id, "reject")}>Deny</Button>
      </div>
    </div>
  );
}

function QuestionFieldView({ info, index, answers, setAnswers }: {
  info: QuestionInfo; index: number; answers: string[][]; setAnswers: (fn: React.SetStateAction<string[][]>) => void;
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
              selected.includes(opt.label) ? "border-primary/40 bg-primary/5 text-foreground" : "border-border/50 text-muted-foreground hover:border-border hover:text-foreground",
            )}
            onClick={() => toggle(opt.label)}
          >
            <span className="text-sm font-medium">{opt.label}</span>
            {opt.description && <span className="text-xs leading-relaxed text-muted-foreground/70">{opt.description}</span>}
          </button>
        ))}
      </div>
      {info.custom && (
        <input
          type="text"
          className="rounded-xl border border-border/50 bg-transparent px-4 py-2.5 text-sm text-foreground placeholder:text-muted-foreground/40 focus:border-border focus:outline-none"
          placeholder="Or type a custom answer..."
          value={customText} onChange={(e) => setCustomText(e.target.value)}
          onKeyDown={(e) => { if (e.key !== "Enter" || !customText.trim()) return; e.preventDefault(); set([customText.trim()]); setCustomText(""); }}
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
  const get = (id: string) => allAnswers[id] ?? [];

  return (
    <div className="flex flex-col gap-5 rounded-2xl border border-border/50 bg-card/50 p-5">
      {requests.flatMap((req) =>
        req.questions.map((q, qi) => (
          <QuestionFieldView
            key={`${req.id}-${qi}`} info={q} index={qi} answers={get(req.id)}
            setAnswers={(fn) => setAllAnswers((prev) => {
              const cur = prev[req.id] ?? emptyAnswers(req);
              return { ...prev, [req.id]: typeof fn === "function" ? fn(cur) : fn };
            })}
          />
        )),
      )}
      <div className="flex gap-2 border-t border-border/30 pt-4">
        <Button size="sm" className="h-9 rounded-xl px-5 text-sm" disabled={!requests.every((r) => get(r.id).every((a) => a.length > 0))} onClick={() => requests.forEach((r) => replyQuestion(r.id, get(r.id)))}>Submit</Button>
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
            value={currentId ?? ""} onChange={(e) => { if (e.target.value) onSelect(e.target.value); }}
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

function Composer({ input, setInput, onSubmit, onAbort, streaming, className }: {
  input: string; setInput: (v: string) => void; onSubmit: () => void; onAbort: () => void;
  streaming: boolean; className?: string;
}) {
  return (
    <div className={cn("mx-auto w-full max-w-3xl", className)}>
      <div className="flex flex-col rounded-2xl border border-border/60 bg-card shadow-lg shadow-black/20 transition-colors focus-within:border-border">
        <textarea
          rows={3}
          className="w-full resize-none bg-transparent px-5 pt-4 pb-1 text-[14px] leading-relaxed text-foreground placeholder:text-muted-foreground/40 focus:outline-none"
          placeholder={streaming ? "Working..." : "What do you want to build?"}
          value={input} onChange={(e) => setInput(e.target.value)} disabled={streaming}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); onSubmit(); } }}
        />
        <div className="flex items-center justify-end px-3 pb-3">
          {streaming ? (
            <button className="flex h-8 w-8 items-center justify-center rounded-full bg-muted text-foreground transition-colors hover:bg-muted-foreground/20" onClick={onAbort}>
              <Square className="h-3.5 w-3.5" />
            </button>
          ) : (
            <button
              className={cn("flex h-8 w-8 items-center justify-center rounded-full transition-all", input.trim() ? "bg-primary text-primary-foreground hover:bg-primary/90" : "bg-muted text-muted-foreground/30")}
              disabled={!input.trim()} onClick={onSubmit}
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
  const { sessionId, sessions, messages, parts, permissions, questions, streaming, error } = useSyncExternalStore(subscribe, getSnapshot);
  const { projectPath } = useProjectContext();
  const [input, setInput] = useState("");

  useEffect(() => { if (projectPath) setCwd(projectPath); }, [projectPath]);

  const submit = async () => {
    if (!input.trim() || streaming) return;
    if (!llmStore.getDefault()) { const ok = await showAISetupDialog(); if (!ok) return; }
    sendMessage(input.trim());
    setInput("");
  };

  const sessionProps = { sessions, currentId: sessionId, onSelect: selectSession, onCreate: createSession };

  if (messages.length + permissions.length + questions.length === 0 && !error) {
    return (
      <div className="relative flex h-full flex-col items-center justify-center overflow-hidden px-6">
        <Logo className="pointer-events-none absolute h-[50%] max-h-[320px] text-white/[0.02]" />
        <div className="z-10 flex w-full max-w-3xl flex-col items-center gap-8">
          <div className="flex flex-col items-center gap-3">
            <h1 className="text-2xl font-medium tracking-tight text-foreground/80">What can I help you build?</h1>
            <p className="text-sm text-muted-foreground/50">Describe what you want to create. I can write code, build features, and help you ship.</p>
          </div>
          <Composer input={input} setInput={setInput} onSubmit={submit} onAbort={abortSession} streaming={streaming} className="w-full" />
        </div>
        <div className="absolute bottom-3 right-3"><SessionSelector {...sessionProps} /></div>
      </div>
    );
  }

  // Build chronological stream of items
  const items: React.ReactNode[] = [];
  for (const msg of messages) {
    const mp = parts.get(msg.id) ?? NO_PARTS;

    if (msg.role === "user") {
      items.push(<MessageBubble key={msg.id} parts={mp} />);
      continue;
    }

    const msgStreaming = streaming && msg === messages[messages.length - 1];
    for (const p of mp) {
      if (p.part_type === "reasoning") items.push(<ThinkingBlock key={p.id} part={p} isStreaming={msgStreaming} />);
      else if (p.part_type === "tool") items.push(<ToolCard key={p.id} part={p} />);
      else if (p.part_type === "text" && p.content) items.push(<div key={p.id} className="text-[14px] leading-[1.7]"><Markdown>{p.content}</Markdown></div>);
    }
    if (msg.error) items.push(<span key={`err-${msg.id}`} className="text-sm text-destructive/80">{msg.error.name ?? "Error"}: {msg.error.message ?? "Unknown error"}</span>);
  }

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center justify-end px-4 py-2"><SessionSelector {...sessionProps} /></div>
      <ChatScrollArea className="min-w-0 flex-1" contentClassName="mx-auto w-full max-w-3xl space-y-3 px-6 py-6">
        {items}
        {permissions.map((perm) => <PermissionCard key={perm.id} perm={perm} />)}
        {questions.length > 0 && <QuestionsPanel requests={questions} />}
        {error && <div className="rounded-xl border border-destructive/20 bg-destructive/5 px-5 py-3 text-sm text-destructive/80">{error}</div>}
      </ChatScrollArea>
      <div className="shrink-0 px-6 pb-5 pt-2">
        <Composer input={input} setInput={setInput} onSubmit={submit} onAbort={abortSession} streaming={streaming} />
      </div>
    </div>
  );
}
