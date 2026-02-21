import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "supported-languages", title: "Supported Languages" },
    { id: "keybindings", title: "Keybindings" },
    { id: "themes", title: "Theme" },
    { id: "opening-files", title: "Opening Files" },
    { id: "multiple-panes", title: "Multiple Tabs" },
];

export default function EditorPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                {/* Breadcrumb */}
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/studio" className="hover:text-foreground transition-colors">Studio IDE</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Code Editor</span>
                </div>

                {/* Header */}
                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Code Editor</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        A full-featured code editor powered by CodeMirror 6 with multi-language support, bracket matching, and a custom dark theme matching the RootCX design system.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        The Studio code editor is built on <strong className="text-foreground font-medium">CodeMirror 6</strong>, the modern, modular text editor framework used by major IDEs and browser-based editors. It is embedded directly in the Studio React frontend via the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">@codemirror/view</code> and language packages.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The editor provides a professional developer experience out of the box, with no configuration required:
                    </p>
                    <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
                        {[
                            "Multi-language syntax highlighting via @codemirror/language family packages",
                            "Line numbers displayed in the gutter on the left",
                            "Bracket matching — matching bracket is highlighted when cursor is adjacent",
                            "Auto-close brackets and quotes for supported languages",
                            "Auto-indentation based on language grammar",
                            "Active line highlighting — the current cursor line has a subtle background",
                            "Rectangular selection with Alt+drag",
                            "Full undo/redo history persisted per file tab",
                            "Keyboard-native navigation and editing shortcuts",
                        ].map((item, i) => (
                            <li key={i} className="flex items-start gap-2">
                                <span className="mt-2 flex-shrink-0 w-1.5 h-1.5 rounded-full bg-primary/60" />
                                {item}
                            </li>
                        ))}
                    </ul>
                </section>

                {/* Supported Languages */}
                <section className="flex flex-col gap-4" id="supported-languages">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Supported Languages</h2>
                    <p className="text-muted-foreground leading-7">
                        Language detection is fully automatic based on the file extension. When you open a file, Studio selects the appropriate CodeMirror language extension and applies syntax highlighting immediately. The following languages are supported:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Language</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Extensions</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">CodeMirror Package</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Notes</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["TypeScript", ".ts, .tsx", "@codemirror/lang-javascript", "Full TS + TSX/JSX support"],
                                    ["JavaScript", ".js, .jsx, .mjs", "@codemirror/lang-javascript", "ES2022+, JSX enabled"],
                                    ["JSON", ".json, .jsonc", "@codemirror/lang-json", "With bracket folding"],
                                    ["SQL", ".sql", "@codemirror/lang-sql", "Standard SQL dialect"],
                                    ["TOML", ".toml", "@codemirror/lang-toml", "Used for Cargo.toml, config"],
                                    ["YAML", ".yml, .yaml", "@codemirror/lang-yaml", "Multi-document support"],
                                    ["Markdown", ".md, .mdx", "@codemirror/lang-markdown", "With inline code highlighting"],
                                    ["CSS", ".css", "@codemirror/lang-css", "Including custom properties"],
                                    ["HTML", ".html, .htm", "@codemirror/lang-html", "With embedded CSS/JS"],
                                    ["Rust", ".rs", "@codemirror/lang-rust", "For Tauri plugin authors"],
                                    ["Plain Text", "(fallback)", "—", "No highlighting, full edit capabilities"],
                                ].map(([lang, exts, pkg, notes], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-medium text-foreground text-xs">{lang}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{exts}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{pkg}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{notes}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <Callout variant="tip" title="Adding language support">
                        If you need syntax highlighting for a language not listed above, CodeMirror 6 has a broad ecosystem of community language packages. Studio's editor is built to be extensible — future releases will expand the built-in language set.
                    </Callout>
                </section>

                {/* Keybindings */}
                <section className="flex flex-col gap-4" id="keybindings">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Keybindings</h2>
                    <p className="text-muted-foreground leading-7">
                        The editor uses <strong className="text-foreground font-medium">CodeMirror's default keymap</strong>, which mirrors the standard conventions familiar from VS Code and other modern editors. On Linux and Windows, substitute <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Ctrl</code> for <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘</code>.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Shortcut (macOS)</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Action</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["⌘S", "Save file to disk"],
                                    ["⌘Z", "Undo"],
                                    ["⌘⇧Z / ⌘Y", "Redo"],
                                    ["⌘F", "Find in current file"],
                                    ["⌘G", "Find next occurrence"],
                                    ["⌘⇧G", "Find previous occurrence"],
                                    ["⌘H", "Find and replace"],
                                    ["⌘A", "Select all"],
                                    ["⌘/", "Toggle line comment"],
                                    ["⌘⇧/", "Toggle block comment"],
                                    ["⌘]", "Indent selection"],
                                    ["⌘[", "Dedent selection"],
                                    ["⌘D", "Select next occurrence of word under cursor"],
                                    ["⌘L", "Select current line"],
                                    ["Alt+↑", "Move line up"],
                                    ["Alt+↓", "Move line down"],
                                    ["⌘⇧K", "Delete current line"],
                                    ["⌘↩", "Insert line below"],
                                    ["⌘⇧↩", "Insert line above"],
                                    ["Home / End", "Move to start / end of line"],
                                    ["⌘Home / ⌘End", "Move to start / end of file"],
                                    ["Ctrl+Space", "Trigger autocompletion (if available)"],
                                ].map(([key, action], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3">
                                            <kbd className="rounded border border-border bg-[#0d0d0d] px-2 py-0.5 font-mono text-xs text-foreground">{key}</kbd>
                                        </td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{action}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* Theme */}
                <section className="flex flex-col gap-4" id="themes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Theme</h2>
                    <p className="text-muted-foreground leading-7">
                        Studio ships a <strong className="text-foreground font-medium">custom dark theme</strong> for CodeMirror that matches the RootCX design system. The theme is defined using CodeMirror's <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">EditorView.theme()</code> API and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">HighlightStyle</code>:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Element</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Color</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Usage</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["Editor background", "#0a0a0a", "Main editor surface"],
                                    ["Gutter background", "#0d0d0d", "Line number gutter"],
                                    ["Active line", "#111111", "Current cursor line highlight"],
                                    ["Selection", "#1a3a5c", "Text selection highlight"],
                                    ["Foreground / text", "#e5e7eb", "Default code text"],
                                    ["Comments", "#6b7280", "Code comments (muted)"],
                                    ["Keywords", "#60a5fa", "if, return, export, const…"],
                                    ["Strings", "#86efac", "String literals"],
                                    ["Numbers", "#f9a8d4", "Numeric literals"],
                                    ["Types / classes", "#a78bfa", "TypeScript types, class names"],
                                    ["Functions", "#fbbf24", "Function names and calls"],
                                    ["Operators", "#94a3b8", "+, -, =, =>, …"],
                                    ["Caret / cursor", "#3b82f6", "Blinking text cursor"],
                                ].map(([elem, color, usage], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 text-foreground text-xs">{elem}</td>
                                        <td className="px-4 py-3">
                                            <div className="flex items-center gap-2">
                                                <span className="w-4 h-4 rounded border border-border/50 flex-shrink-0" style={{ backgroundColor: color }} />
                                                <code className="font-mono text-xs text-muted-foreground">{color}</code>
                                            </div>
                                        </td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{usage}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <p className="text-muted-foreground leading-7">
                        The theme is not user-configurable in the current release. A custom theme API is planned for a future Studio version.
                    </p>
                </section>

                {/* Opening Files */}
                <section className="flex flex-col gap-4" id="opening-files">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Opening Files</h2>
                    <p className="text-muted-foreground leading-7">
                        There are three ways to open a file in the editor:
                    </p>
                    <ol className="flex flex-col gap-3 text-muted-foreground text-sm leading-7 list-none">
                        {[
                            ["Click in the File Explorer", "Single-click any file in the left sidebar to open it in the editor. If a tab for that file already exists, it is focused instead of opening a duplicate."],
                            ["Command Palette (⌘K → Open File…)", "Type the filename or a partial path to fuzzy-find any file in the workspace. Press Enter to open."],
                            ["File Switcher (⌘P)", "Opens a quick-pick overlay showing recently opened files. Useful for jumping between files you've already visited."],
                        ].map(([title, desc], i) => (
                            <li key={i} className="flex gap-3">
                                <span className="flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-full bg-primary/10 text-primary text-xs font-bold mt-0.5">{i + 1}</span>
                                <span><strong className="text-foreground font-medium">{title}:</strong> {desc}</span>
                            </li>
                        ))}
                    </ol>
                    <p className="text-muted-foreground leading-7">
                        Files are read from disk when opened. If the file is modified externally (e.g., by another editor or a script), Studio detects the change and prompts you to reload.
                    </p>
                </section>

                {/* Multiple Tabs */}
                <section className="flex flex-col gap-4" id="multiple-panes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Multiple Tabs</h2>
                    <p className="text-muted-foreground leading-7">
                        The editor supports multiple open files simultaneously as <strong className="text-foreground font-medium">tabs</strong> displayed along the top of the editor area. Each tab represents one open file and maintains its own independent editor state — scroll position, selection, undo history, and unsaved changes.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Tab behavior:
                    </p>
                    <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
                        {[
                            "Click a tab to switch to that file instantly",
                            "Unsaved files display a small dot indicator on their tab",
                            "Close a tab with the × button on the tab, or with ⌘W",
                            "Closing an unsaved tab prompts Save / Discard / Cancel",
                            "Tabs persist across Studio sessions — your open files are restored on next launch",
                            "The active tab is highlighted with the primary accent color",
                        ].map((item, i) => (
                            <li key={i} className="flex items-start gap-2">
                                <span className="mt-2 flex-shrink-0 w-1.5 h-1.5 rounded-full bg-primary/60" />
                                {item}
                            </li>
                        ))}
                    </ul>
                    <Callout variant="info" title="Split panes">
                        Split-pane editing (side-by-side files) is not yet supported in the current Studio release. It is on the roadmap. For now, use tab switching to work across multiple files.
                    </Callout>
                </section>

                {/* Status Bar */}
                <section className="flex flex-col gap-4" id="status-bar">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Editor Status Bar</h2>
                    <p className="text-muted-foreground leading-7">
                        A thin status bar beneath the editor displays context for the currently active file:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Item</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Example</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Description</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["Language", "TypeScript", "Detected language for the current file"],
                                    ["Cursor position", "Ln 42, Col 18", "Line number and column of the text cursor"],
                                    ["Encoding", "UTF-8", "File encoding (always UTF-8 in Studio)"],
                                    ["EOL", "LF / CRLF", "Line ending format of the file"],
                                    ["File path", "index.ts", "Filename or relative path in the workspace"],
                                ].map(([item, example, desc], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-medium text-foreground text-xs">{item}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{example}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{desc}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* Example */}
                <section className="flex flex-col gap-4" id="example">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Example: Worker in the Editor</h2>
                    <p className="text-muted-foreground leading-7">
                        Here is a representative worker file as it appears with full TypeScript syntax highlighting in Studio:
                    </p>
                    <CodeBlock language="typescript" filename="index.ts" code={`import type { Env } from "./types";

/**
 * Handle RPC calls from the RootCX runtime.
 * @param method - The RPC method name
 * @param params - Arbitrary JSON parameters
 * @param caller - Authenticated caller info (if auth enabled)
 */
export async function handleRpc(
  method: string,
  params: Record<string, unknown>,
  caller?: { userId: string; username: string }
): Promise<unknown> {
  switch (method) {
    case "greet": {
      const name = params.name as string;
      return { message: \`Hello, \${name}! Called by \${caller?.username ?? "anonymous"}\` };
    }
    case "createInvoice": {
      const { contactId, amount, currency } = params;
      // Business logic here
      const invoice = await generateInvoice({ contactId, amount, currency });
      return { invoiceId: invoice.id, status: "created" };
    }
    default:
      throw new Error(\`Unknown RPC method: \${method}\`);
  }
}

/**
 * Handle background jobs dispatched via POST /api/v1/apps/{appId}/jobs
 */
export async function handleJob(payload: Record<string, unknown>): Promise<void> {
  const { type, data } = payload;
  if (type === "send-email") {
    await sendEmail(data as EmailPayload);
  }
}`} />
                </section>

                <PageNav href="/studio/editor" />
            </div>
        </DocsLayout>
    );
}
