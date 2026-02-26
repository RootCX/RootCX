import { useState, useRef, useEffect, useSyncExternalStore } from "react";
import {
  subscribe, getSnapshot, sendMessage, abortSession,
  createSession, selectSession, replyPermission,
  replyQuestion, rejectQuestion, setCwd,
  type QuestionRequest, type QuestionInfo,
  type Message, type Part, type Permission, type Session,
} from "./store";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { useProjectContext } from "@/components/layout/app-context";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { aiConfigStore } from "@/lib/ai-models";
import { ArrowUp, Square, Plus, ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";
import Markdown from "react-markdown";

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

const NO_PARTS: Part[] = [];
const TOOL_LABELS: Record<string, string> = { read: "Reading", write: "Writing", edit: "Editing", bash: "Running command", grep: "Searching", glob: "Finding files", ls: "Browsing" };
const FADE_MASK = "linear-gradient(to bottom, transparent 0%, black 12px, black calc(100% - 12px), transparent 100%)";
const toolTitle = (t: Part) => t.tool_state?.title ?? TOOL_LABELS[t.tool_name ?? ""] ?? t.tool_name;

function PartView({ part }: { part: Part }) {
  if (part.part_type !== "text" && part.part_type !== "reasoning") return null;
  return (
    <div className={cn("break-words text-[14px] leading-[1.7]", part.part_type === "reasoning" && "italic text-muted-foreground/80")}>
      <Markdown components={mdComponents}>{part.content}</Markdown>
    </div>
  );
}

function useCinematicScroll(ref: React.RefObject<HTMLDivElement | null>, enabled: boolean, deps: unknown[]) {
  const raf = useRef(0);
  useEffect(() => {
    const el = ref.current;
    if (!el || !enabled) return;
    const start = el.scrollTop, distance = el.scrollHeight - el.clientHeight - start;
    if (distance <= 0) return;
    cancelAnimationFrame(raf.current);
    const dur = Math.min(1800, Math.max(800, distance * 12));
    let t0: number | null = null;
    const step = (now: number) => {
      if (!t0) t0 = now;
      const p = Math.min((now - t0) / dur, 1);
      el.scrollTop = start + distance * (1 - (1 - p) ** 4);
      if (p < 1) raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

function ToolDetailView({ part }: { part: Part }) {
  if (!part.tool_input && !part.content) return null;
  const inp = part.tool_input ?? {};
  const summary = (inp.command as string) ? `$ ${inp.command}`
    : (inp.pattern as string) ? `/${inp.pattern}/`
    : (inp.file_path as string) ?? (inp.path as string) ?? null;
  const lines = part.tool_state?.status !== "running" && part.content ? part.content.split("\n") : null;
  const preview = lines ? (lines.length > 8 ? lines.slice(0, 8).join("\n") + "\n…" : part.content) : null;
  return (
    <div className="mx-3 mb-1.5 rounded-lg bg-[#141414] px-3 py-2 text-[11px] font-mono text-muted-foreground/60">
      {summary && <div className="truncate">{summary}</div>}
      {preview && <pre className="mt-1 max-h-[120px] overflow-y-auto whitespace-pre-wrap break-all leading-relaxed opacity-60">{preview}</pre>}
    </div>
  );
}

function MiniToolCard({ tools }: { tools: Part[] }) {
  const [expanded, setExpanded] = useState(false);
  const errored = tools.some((t) => t.tool_state?.status === "error");
  return (
    <div className="-mt-3 animate-[settle_0.5s_cubic-bezier(0.16,1,0.3,1)_both]">
      <button
        onClick={() => setExpanded((e) => !e)}
        className={cn("flex items-center gap-2 rounded-lg px-3 py-1.5 text-xs transition-colors", errored ? "text-destructive/60 hover:text-destructive/80" : "text-muted-foreground/50 hover:text-muted-foreground/70")}
      >
        <span className={cn("h-1.5 w-1.5 rounded-full", errored ? "bg-destructive" : "bg-green-500")} />
        {tools.length} operations
        <ChevronDown className={cn("h-3 w-3 transition-transform duration-200", expanded && "rotate-180")} />
      </button>
      {expanded && (
        <div className="mt-1 max-h-[300px] overflow-y-auto rounded-xl border border-border/30 bg-card/20 py-0.5">
          {tools.map((tool) => (
            <div key={tool.id}>
              <div className="px-3 py-1 text-xs truncate text-muted-foreground/60">{toolTitle(tool)}</div>
              <ToolDetailView part={tool} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
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
        ) : msg.role === "assistant" && msg.error ? (
          <span className="text-sm text-destructive/80">{msg.error.name ?? "Error"}: {msg.error.message ?? "Unknown error"}</span>
        ) : null}
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
      {info.custom !== false && (
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
  const get = (r: QuestionRequest) => allAnswers[r.id] ?? emptyAnswers(r);

  return (
    <div className="flex flex-col gap-5 rounded-2xl border border-border/50 bg-card/50 p-5">
      {requests.flatMap((req) =>
        req.questions.map((q, qi) => (
          <QuestionFieldView
            key={`${req.id}-${qi}`} info={q} index={qi} answers={get(req)}
            setAnswers={(fn) => setAllAnswers((prev) => ({ ...prev, [req.id]: typeof fn === "function" ? fn(get(req)) : fn }))}
          />
        )),
      )}
      <div className="flex gap-2 border-t border-border/30 pt-4">
        <Button size="sm" className="h-9 rounded-xl px-5 text-sm" disabled={!requests.every((r) => get(r).every((a) => a.length > 0))} onClick={() => requests.forEach((r) => replyQuestion(r.id, get(r)))}>Submit</Button>
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

function Composer({ input, setInput, onSubmit, onAbort, streaming, liveTools, className }: {
  input: string; setInput: (v: string) => void; onSubmit: () => void; onAbort: () => void;
  streaming: boolean; liveTools: Part[]; className?: string;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const live = streaming && liveTools.length > 0;

  useCinematicScroll(scrollRef, live, [liveTools.length, liveTools[liveTools.length - 1]?.tool_state?.status]);

  return (
    <div className={cn("mx-auto w-full max-w-3xl", className)}>
      <div className={cn("flex flex-col rounded-2xl border bg-card shadow-lg shadow-black/20 transition-colors", live ? "border-primary/30" : "border-border/60 focus-within:border-border")}>
        {live ? (
          <div
            ref={scrollRef}
            className="max-h-[100px] overflow-y-hidden overflow-x-hidden pt-3"
            style={{ maskImage: FADE_MASK, WebkitMaskImage: FADE_MASK }}
          >
            {liveTools.map((tool, i) => (
              <div key={tool.id} className={cn("px-5 py-1 text-xs truncate text-muted-foreground transition-opacity duration-700", tool.tool_state?.status === "completed" && i !== liveTools.length - 1 && "opacity-40")}>
                {toolTitle(tool)}
              </div>
            ))}
          </div>
        ) : (
          <textarea
            rows={3}
            className="w-full resize-none bg-transparent px-5 pt-4 pb-1 text-[14px] leading-relaxed text-foreground placeholder:text-muted-foreground/40 focus:outline-none"
            placeholder={streaming ? "Thinking..." : "What do you want to build?"}
            value={input} onChange={(e) => setInput(e.target.value)} disabled={streaming}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); onSubmit(); } }}
          />
        )}
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
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => { if (projectPath) setCwd(projectPath); }, [projectPath]);
  useEffect(() => { endRef.current?.scrollIntoView({ behavior: "smooth" }); }, [messages, parts, questions, permissions]);

  const submit = async () => {
    if (!input.trim() || streaming) return;
    if (!aiConfigStore.getSnapshot()) { const ok = await showAISetupDialog(); if (!ok) return; }
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
          <Composer input={input} setInput={setInput} onSubmit={submit} onAbort={abortSession} streaming={streaming} liveTools={[]} className="w-full" />
        </div>
        <div className="absolute bottom-3 right-3"><SessionSelector {...sessionProps} /></div>
      </div>
    );
  }

  const items: React.ReactNode[] = [];
  let turnTools: Part[] = [], turnTexts: React.ReactNode[] = [];
  let liveTools: Part[] = [];
  const flush = (live: boolean) => {
    if (live) { liveTools = [...turnTools]; }
    else if (turnTools.length) items.push(<MiniToolCard key={`tc-${turnTools[0].id}`} tools={[...turnTools]} />);
    items.push(...turnTexts);
    turnTools = []; turnTexts = [];
  };
  for (const msg of messages) {
    const mp = parts.get(msg.id) ?? NO_PARTS;
    if (msg.role === "user") { flush(false); items.push(<MessageView key={msg.id} msg={msg} parts={mp} />); }
    else {
      const text = mp.filter((p) => p.part_type !== "tool"), tool = mp.filter((p) => p.part_type === "tool");
      if (text.length || msg.error) turnTexts.push(<MessageView key={msg.id} msg={msg} parts={text} />);
      turnTools.push(...tool);
    }
  }
  flush(streaming);

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center justify-end px-4 py-2"><SessionSelector {...sessionProps} /></div>
      <div className="min-h-0 min-w-0 flex-1 overflow-y-auto overflow-x-hidden">
        <div className="mx-auto w-full max-w-3xl space-y-5 px-6 py-6">
          {items}
          {permissions.map((perm) => <PermissionCard key={perm.id} perm={perm} />)}
          {questions.length > 0 && <QuestionsPanel requests={questions} />}
          {error && <div className="rounded-xl border border-destructive/20 bg-destructive/5 px-5 py-3 text-sm text-destructive/80">{error}</div>}
          <div ref={endRef} />
        </div>
      </div>
      <div className="shrink-0 px-6 pb-5 pt-2">
        <Composer input={input} setInput={setInput} onSubmit={submit} onAbort={abortSession} streaming={streaming} liveTools={liveTools} />
      </div>
    </div>
  );
}
