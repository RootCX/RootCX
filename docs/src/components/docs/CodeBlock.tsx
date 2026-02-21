"use client";

import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "@/lib/utils";

interface CodeBlockProps {
    code: string;
    language?: string;
    filename?: string;
    className?: string;
}

export function CodeBlock({ code, language = "bash", filename, className }: CodeBlockProps) {
    const [copied, setCopied] = useState(false);

    const copy = async () => {
        await navigator.clipboard.writeText(code.trim());
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
    };

    return (
        <div className={cn("group relative rounded-lg border border-border bg-[#111] overflow-hidden my-4", className)}>
            <div className="flex items-center justify-between border-b border-border bg-[#0d0d0d] px-4 py-2">
                <div className="flex items-center gap-2">
                    {filename && (
                        <span className="font-mono text-xs text-muted-foreground">{filename}</span>
                    )}
                    {!filename && (
                        <span className="text-xs font-semibold uppercase tracking-widest text-muted-foreground/60">
                            {language}
                        </span>
                    )}
                </div>
                <button
                    onClick={copy}
                    className="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
                >
                    {copied ? (
                        <Check className="h-3.5 w-3.5 text-green-400" />
                    ) : (
                        <Copy className="h-3.5 w-3.5" />
                    )}
                    <span>{copied ? "Copied" : "Copy"}</span>
                </button>
            </div>
            <pre className="overflow-x-auto p-4 text-sm leading-relaxed">
                <code className="font-mono text-foreground/90 whitespace-pre">{code.trim()}</code>
            </pre>
        </div>
    );
}

interface TerminalBlockProps {
    commands: string | string[];
    className?: string;
}

export function TerminalBlock({ commands, className }: TerminalBlockProps) {
    const lines = Array.isArray(commands) ? commands : [commands];
    const [copied, setCopied] = useState(false);

    const copy = async () => {
        await navigator.clipboard.writeText(lines.join("\n"));
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
    };

    return (
        <div className={cn("group relative rounded-lg border border-border bg-[#111] overflow-hidden my-4", className)}>
            <div className="flex items-center justify-between border-b border-border bg-[#0d0d0d] px-4 py-2">
                <span className="text-xs font-semibold uppercase tracking-widest text-muted-foreground/60">Terminal</span>
                <button
                    onClick={copy}
                    className="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
                >
                    {copied ? (
                        <Check className="h-3.5 w-3.5 text-green-400" />
                    ) : (
                        <Copy className="h-3.5 w-3.5" />
                    )}
                    <span>{copied ? "Copied" : "Copy"}</span>
                </button>
            </div>
            <div className="p-4 flex flex-col gap-2">
                {lines.map((line, i) => (
                    <div key={i} className="flex items-start gap-2 font-mono text-sm">
                        <span className="select-none text-primary">$</span>
                        <span className="text-foreground/90 whitespace-pre">{line}</span>
                    </div>
                ))}
            </div>
        </div>
    );
}
