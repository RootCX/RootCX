import { useState, useCallback, useEffect, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";

interface DependsOn { key: string; equals: boolean | string }

interface Question {
    key: string;
    label: string;
    question_type: { kind: string; options?: { value: string; label: string }[] };
    default?: boolean | string;
    depends_on?: DependsOn | null;
}

interface WizardResult {
    path: string;
    name: string;
    presetId: string;
    answers: Record<string, boolean | string>;
}

let resolve: ((r: WizardResult | null) => void) | null = null;
let setOpen: ((v: boolean) => void) | null = null;

export function showScaffoldWizard(): Promise<WizardResult | null> {
    return new Promise((res) => {
        resolve = res;
        setOpen?.(true);
    });
}

export function ScaffoldWizardPortal() {
    const [open, _setOpen] = useState(false);
    useEffect(() => { setOpen = _setOpen; return () => { setOpen = null; }; }, []);
    if (!open) return null;
    return createPortal(
        <Wizard onDone={(r) => { _setOpen(false); resolve?.(r); resolve = null; }} />,
        document.body,
    );
}

function resolveVisible(questions: Question[], answers: Record<string, boolean | string>) {
    const effective = { ...answers };
    for (;;) {
        const visible = questions.filter(
            q => !q.depends_on || effective[q.depends_on.key] === q.depends_on.equals,
        );
        const keys = new Set(visible.map(q => q.key));
        let pruned = false;
        for (const k of Object.keys(effective))
            if (!keys.has(k)) { delete effective[k]; pruned = true; }
        if (!pruned) return { visible, cleaned: effective };
    }
}

function Wizard({ onDone }: { onDone: (r: WizardResult | null) => void }) {
    const [step, setStep] = useState(0); // 0=name, 1+=question index into visibleQs
    const [name, setName] = useState("my-project");
    const [path, setPath] = useState("");
    const [allQuestions, setAllQuestions] = useState<Question[]>([]);
    const [answers, setAnswers] = useState<Record<string, boolean | string>>({});
    const inputRef = useRef<HTMLInputElement>(null);

    const presetId = "blank";
    const cancel = useCallback(() => onDone(null), [onDone]);

    const visibleQs = useMemo(
        () => resolveVisible(allQuestions, answers).visible,
        [allQuestions, answers],
    );

    useEffect(() => { inputRef.current?.focus(); }, [step]);

    const submitName = useCallback(async () => {
        if (!name.trim()) return;
        const dir = await open({ directory: true, title: "Choose location" });
        if (!dir) return;
        setPath(`${dir}/${name}`);
        const qs = await invoke<Question[]>("get_preset_questions", { presetId });
        const defaults: Record<string, boolean | string> = {};
        for (const q of qs) if (q.default != null) defaults[q.key] = q.default;
        setAllQuestions(qs);
        setAnswers(defaults);
        setStep(1);
    }, [name, presetId]);

    const advance = useCallback((fromIndex: number, newAnswers: Record<string, boolean | string>) => {
        const { visible: nextVisible, cleaned: nextCleaned } = resolveVisible(allQuestions, newAnswers);
        if (fromIndex + 1 >= nextVisible.length) {
            onDone({ path, name, presetId, answers: nextCleaned });
        } else {
            setStep(fromIndex + 2); // +1 next question, +1 because step 0 = name
        }
    }, [allQuestions, path, name, presetId, onDone]);

    const q = visibleQs[step - 1];
    const total = 1 + visibleQs.length;

    return (
        <div className="fixed inset-0 z-50 flex items-start justify-center pt-[20vh]" onClick={cancel}>
            <div className="absolute inset-0 bg-black/50" />
            <div
                className="relative w-full max-w-md min-h-[120px] rounded-lg border border-border bg-card shadow-2xl"
                onClick={(e) => e.stopPropagation()}
                onKeyDown={(e) => e.key === "Escape" && cancel()}
            >
                {/* Thin progress track */}
                <div className="h-px bg-border">
                    <div
                        className="h-px bg-primary/60 transition-all duration-300"
                        style={{ width: `${(step / total) * 100}%` }}
                    />
                </div>

                {/* ── Step 0: Project name ── */}
                {step === 0 && (
                    <div className="px-3 py-2">
                        <div className="flex items-center text-xs text-muted-foreground mb-1.5">
                            <span>New Project</span>
                            <span className="ml-auto opacity-50">↵ to continue</span>
                        </div>
                        <input
                            ref={inputRef}
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value.replace(/\s/g, "-"))}
                            onKeyDown={(e) => e.key === "Enter" && submitName()}
                            placeholder="project-name"
                            className="w-full bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none"
                        />
                    </div>
                )}

                {/* ── Step 1+: Questions ── */}
                {step > 0 && q && (
                    <div className="px-3 py-2">
                        <div className="flex items-center text-xs text-muted-foreground mb-2">
                            <span>{q.label}</span>
                            <span className="ml-auto opacity-40">{step}/{visibleQs.length}</span>
                        </div>

                        {q.question_type.kind === "bool" && (
                            <div className="flex gap-3">
                                {([true, false] as const).map((val) => (
                                    <Button
                                        key={String(val)}
                                        variant="outline"
                                        onClick={() => {
                                            const next = { ...answers, [q.key]: val };
                                            setAnswers(next);
                                            setTimeout(() => advance(step - 1, next), 150);
                                        }}
                                    >
                                        {val ? "Yes" : "No"}
                                    </Button>
                                ))}
                            </div>
                        )}

                        {q.question_type.kind === "text" && (
                            <input
                                ref={inputRef}
                                type="text"
                                value={(answers[q.key] as string) ?? ""}
                                onChange={(e) => setAnswers((a) => ({ ...a, [q.key]: e.target.value }))}
                                onKeyDown={(e) => {
                                    if (e.key !== "Enter") return;
                                    advance(step - 1, answers);
                                }}
                                className="w-full bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none"
                            />
                        )}

                        {q.question_type.kind === "choice" && q.question_type.options && (
                            <div className="flex gap-3">
                                {q.question_type.options.map((opt) => (
                                    <Button
                                        key={opt.value}
                                        variant="outline"
                                        onClick={() => {
                                            const next = { ...answers, [q.key]: opt.value };
                                            setAnswers(next);
                                            setTimeout(() => advance(step - 1, next), 150);
                                        }}
                                    >
                                        {opt.label}
                                    </Button>
                                ))}
                            </div>
                        )}

                    </div>
                )}
            </div>
        </div>
    );
}
