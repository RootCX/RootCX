import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "getting-started", title: "Getting Started" },
    { id: "panels", title: "Panels Overview" },
    { id: "file-explorer", title: "File Explorer" },
    { id: "editor", title: "Code Editor" },
    { id: "forge", title: "Forge Panel" },
    { id: "console", title: "Console Panel" },
    { id: "command-palette", title: "Command Palette" },
    { id: "keyboard-shortcuts", title: "Keyboard Shortcuts" },
];

export default function StudioPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                {/* Breadcrumb */}
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/studio" className="hover:text-foreground transition-colors">Studio IDE</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Overview</span>
                </div>

                {/* Header */}
                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Studio IDE</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        A local-first desktop IDE for building, deploying, and managing RootCX applications.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX Studio is a native desktop application built with <strong className="text-foreground font-medium">Tauri v2</strong> — a Rust shell hosting a React 19 frontend. It provides a fully integrated development environment purpose-built for RootCX applications, combining a code editor, deployment tooling, live logs, and Core controls in a single window.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Studio is the recommended way to develop RootCX applications locally. It automatically manages the Core daemon lifecycle, so you never need to start or stop PostgreSQL or the HTTP API manually. When Studio opens, the daemon starts. When Studio closes, the daemon stops.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Layer</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Technology</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Role</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["Shell", "Tauri v2 (Rust)", "Native window, OS integration, binary sidecar management"],
                                    ["Frontend", "React 19 + TypeScript", "All UI panels, routing, state management"],
                                    ["Styling", "Tailwind CSS v4", "Design system, theming, responsive layout"],
                                    ["Editor", "CodeMirror 6", "Embedded code editor with syntax highlighting"],
                                    ["Icons", "Lucide React", "Consistent icon set throughout the interface"],
                                    ["IPC", "Tauri Commands", "Frontend ↔ Rust shell communication"],
                                ].map(([layer, tech, role], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-medium text-foreground text-xs">{layer}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{tech}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{role}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <Callout variant="info" title="Studio bundles the Core binary">
                        Studio ships with the RootCX Core binary embedded as a Tauri sidecar. You do not need to install Core separately when using Studio. The correct binary for your platform is bundled automatically at build time.
                    </Callout>
                </section>

                {/* Getting Started */}
                <section className="flex flex-col gap-4" id="getting-started">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Getting Started</h2>
                    <p className="text-muted-foreground leading-7">
                        Download the latest Studio release for your platform from the <strong className="text-foreground font-medium">GitHub Releases</strong> page. Studio is distributed as a platform-native installer:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Platform</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Format</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Filename</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["macOS (Apple Silicon)", ".dmg", "RootCX-Studio_x.y.z_aarch64.dmg"],
                                    ["macOS (Intel)", ".dmg", "RootCX-Studio_x.y.z_x86_64.dmg"],
                                    ["Linux x86_64", ".AppImage / .deb", "rootcx-studio_x.y.z_amd64.AppImage"],
                                    ["Windows x86_64", ".msi", "RootCX-Studio_x.y.z_x64_en-US.msi"],
                                ].map(([platform, fmt, filename], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 text-foreground text-xs">{platform}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{fmt}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{filename}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <p className="text-muted-foreground leading-7">
                        After installation, launch Studio from your Applications folder or Start Menu. On first launch:
                    </p>
                    <ol className="flex flex-col gap-3 text-muted-foreground text-sm leading-7 list-none">
                        {[
                            ["Studio starts", "The Tauri shell initializes and renders the React frontend."],
                            ["Core daemon starts", "Studio spawns the bundled rootcx-core binary as a sidecar process."],
                            ["Health check loop", "Studio polls GET /health every second until the daemon reports {\"status\":\"ok\"}."],
                            ["Status bar updates", "Once healthy, the status bar shows green indicators for PostgreSQL and Core."],
                            ["Ready", "You can now install apps, write code, and deploy backend processes."],
                        ].map(([title, desc], i) => (
                            <li key={i} className="flex gap-3">
                                <span className="flex-shrink-0 flex items-center justify-center w-6 h-6 rounded-full bg-primary/10 text-primary text-xs font-bold mt-0.5">{i + 1}</span>
                                <span><strong className="text-foreground font-medium">{title}:</strong> {desc}</span>
                            </li>
                        ))}
                    </ol>
                    <Callout variant="tip" title="First-time setup">
                        On macOS, you may need to right-click the app and choose Open the first time to bypass Gatekeeper. On subsequent launches, Studio opens normally.
                    </Callout>
                </section>

                {/* Panels Overview */}
                <section className="flex flex-col gap-4" id="panels">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Panels Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        Studio is organized into a set of panels arranged in a fixed layout. Each panel is dedicated to a specific workflow area. Panels cannot be rearranged in the current release; the layout is intentionally minimal and task-focused.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Panel</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Position</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Purpose</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["File Explorer", "Left sidebar", "Browse and open workspace files"],
                                    ["Code Editor", "Center main area", "Edit TypeScript, JSON, SQL, and other files"],
                                    ["Forge", "Right sidebar", "Deploy backends and manage backend lifecycle"],
                                    ["Console", "Bottom drawer", "Live log stream from running backends"],
                                    ["Command Palette", "Floating overlay (⌘K)", "Keyboard-driven access to all commands"],
                                    ["Status Bar", "Bottom strip", "PostgreSQL, Core, and Forge health indicators"],
                                ].map(([panel, pos, purpose], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-medium text-foreground text-xs">{panel}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{pos}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{purpose}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* File Explorer */}
                <section className="flex flex-col gap-4" id="file-explorer">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">File Explorer</h2>
                    <p className="text-muted-foreground leading-7">
                        The File Explorer occupies the left sidebar and displays the contents of your current <strong className="text-foreground font-medium">workspace directory</strong>. A workspace is simply a folder on your local filesystem containing your backend code, manifest, and any other project files.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        By default, Studio uses <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/RootCX/apps/{"{appId}"}/</code> as the workspace for each app. You can open any folder as a workspace using the command palette.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Files and directories are displayed in a tree view. Clicking a file opens it in the Code Editor. The tree automatically refreshes when files change on disk, so external edits (from your terminal, VS Code, etc.) are reflected immediately.
                    </p>
                    <div className="rounded-lg border border-border bg-[#0d0d0d] p-4">
                        <p className="text-xs font-mono text-muted-foreground mb-3">Typical workspace layout:</p>
                        <CodeBlock language="text" code={`my-app/
├── index.ts          ← Backend entry point
├── package.json      ← npm dependencies
├── manifest.json     ← App manifest
├── lib/
│   ├── email.ts
│   └── utils.ts
└── .env              ← Local env (not deployed)`} />
                    </div>
                </section>

                {/* Code Editor */}
                <section className="flex flex-col gap-4" id="editor">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Code Editor</h2>
                    <p className="text-muted-foreground leading-7">
                        The Code Editor is powered by <strong className="text-foreground font-medium">CodeMirror 6</strong> and occupies the main center area of the Studio window. It provides a professional editing experience with full multi-language syntax highlighting, line numbers, bracket matching, and auto-indentation.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Files open as tabs across the top of the editor. You can have multiple files open simultaneously and switch between them by clicking tabs. Unsaved files are indicated with a dot indicator on their tab. Press <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘S</code> to save the current file.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Language detection is automatic based on the file extension. See the <Link href="/studio/editor" className="text-primary hover:underline">Code Editor reference</Link> for the full list of supported languages and keybindings.
                    </p>
                </section>

                {/* Forge Panel */}
                <section className="flex flex-col gap-4" id="forge">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Forge Panel</h2>
                    <p className="text-muted-foreground leading-7">
                        The <strong className="text-foreground font-medium">Forge panel</strong> is Studio's deployment center, located in the right sidebar. It shows the current backend status for the active app and provides controls to deploy, start, and stop the backend process.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        When you click <strong className="text-foreground font-medium">Deploy</strong>, Studio packages your workspace as a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.tar.gz</code> archive, uploads it to the Core API, and Core extracts, installs dependencies, and spawns the backend process. The deployment log in the Forge panel shows each step as it happens.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        See the <Link href="/studio/forge" className="text-primary hover:underline">Forge reference</Link> for the full deployment flow and backend requirements.
                    </p>
                </section>

                {/* Console Panel */}
                <section className="flex flex-col gap-4" id="console">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Console Panel</h2>
                    <p className="text-muted-foreground leading-7">
                        The Console panel streams real-time log output from your running backends. It connects to the Core API's <strong className="text-foreground font-medium">Server-Sent Events (SSE)</strong> log endpoint at <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET /api/v1/apps/{"{appId}"}/logs</code> and displays each log line as it arrives.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Log lines are color-coded by severity level:
                    </p>
                    <div className="flex flex-col gap-2">
                        {[
                            ["info", "text-blue-400", "bg-blue-500/10 border-blue-500/20", "Standard informational output from your backend"],
                            ["warn", "text-yellow-400", "bg-yellow-500/10 border-yellow-500/20", "Non-fatal warnings — backend continues running"],
                            ["error", "text-red-400", "bg-red-500/10 border-red-500/20", "Errors — review immediately, may indicate a crash"],
                        ].map(([level, textColor, badgeStyle, desc]) => (
                            <div key={level} className="flex items-center gap-3 rounded-lg border border-border/50 bg-[#0d0d0d] px-4 py-3">
                                <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-bold font-mono border ${badgeStyle} ${textColor}`}>{level}</span>
                                <span className="text-sm text-muted-foreground">{desc}</span>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        Each log line includes a <strong className="text-foreground font-medium">timestamp</strong> in ISO 8601 format. The console auto-scrolls to the latest line. Use the <strong className="text-foreground font-medium">Clear</strong> button to wipe the current console buffer without affecting the backend. Toggle the console with <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘`</code>.
                    </p>
                </section>

                {/* Command Palette */}
                <section className="flex flex-col gap-4" id="command-palette">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Command Palette</h2>
                    <p className="text-muted-foreground leading-7">
                        Press <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘K</code> (macOS) or <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Ctrl+K</code> (Linux/Windows) to open the Command Palette — a floating search overlay that gives you keyboard-driven access to every Studio operation.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Type any part of a command name to filter the list. Use <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">↑</code> / <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">↓</code> to navigate, and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Enter</code> to execute. Press <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Escape</code> to close.
                    </p>
                    <p className="text-muted-foreground leading-7">Available commands include:</p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Command</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Description</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["Open File…", "Quick-open any file in the workspace by name"],
                                    ["Deploy Backend", "Trigger a fresh package and deployment of the current workspace"],
                                    ["Start Backend", "Start the backend without re-deploying"],
                                    ["Stop Backend", "Gracefully stop the running backend"],
                                    ["Clear Console", "Wipe the console log buffer"],
                                    ["Open Settings", "Open the Studio settings panel"],
                                    ["Open Workspace…", "Choose a different folder as the workspace"],
                                    ["Install App from Manifest…", "POST a manifest.json to Core to install or update an app"],
                                ].map(([cmd, desc], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{cmd}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{desc}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* Keyboard Shortcuts */}
                <section className="flex flex-col gap-4" id="keyboard-shortcuts">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Keyboard Shortcuts</h2>
                    <p className="text-muted-foreground leading-7">
                        Studio registers global keyboard shortcuts for the most common actions. On Linux and Windows, substitute <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Ctrl</code> for <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘</code>.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Shortcut (macOS)</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Shortcut (Win/Linux)</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Action</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["⌘K", "Ctrl+K", "Open command palette"],
                                    ["⌘S", "Ctrl+S", "Save current file"],
                                    ["⌘P", "Ctrl+P", "Open file switcher (quick-open)"],
                                    ["⌘`", "Ctrl+`", "Toggle console panel"],
                                    ["⌘Z", "Ctrl+Z", "Undo in editor"],
                                    ["⌘⇧Z", "Ctrl+Shift+Z", "Redo in editor"],
                                    ["⌘F", "Ctrl+F", "Find in current file"],
                                    ["⌘/", "Ctrl+/", "Toggle line comment in editor"],
                                    ["⌘⇧P", "Ctrl+Shift+P", "Deploy backend (Forge)"],
                                ].map(([mac, win, action], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3">
                                            <kbd className="rounded border border-border bg-[#0d0d0d] px-2 py-0.5 font-mono text-xs text-foreground">{mac}</kbd>
                                        </td>
                                        <td className="px-4 py-3">
                                            <kbd className="rounded border border-border bg-[#0d0d0d] px-2 py-0.5 font-mono text-xs text-foreground">{win}</kbd>
                                        </td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{action}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* Core Status */}
                <section className="flex flex-col gap-4" id="core-status">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Core Status Bar</h2>
                    <p className="text-muted-foreground leading-7">
                        The status bar at the very bottom of the Studio window shows real-time health indicators for the three main subsystems. Each indicator has a colored dot:
                    </p>
                    <div className="flex flex-col gap-2">
                        {[
                            ["PostgreSQL", "green", "Embedded PostgreSQL is running and accepting connections on port 5480"],
                            ["Core", "green", "Core HTTP API is up and responding to requests on port 9100"],
                            ["Forge", "grey", "Backend status: green = running, grey = stopped, red = crashed"],
                        ].map(([label, _color, desc]) => (
                            <div key={label} className="flex items-start gap-3 rounded-lg border border-border/50 bg-[#0d0d0d] px-4 py-3">
                                <span className="mt-1.5 flex-shrink-0 w-2 h-2 rounded-full bg-green-500" />
                                <div>
                                    <p className="text-sm font-medium text-foreground">{label}</p>
                                    <p className="text-xs text-muted-foreground mt-0.5">{desc}</p>
                                </div>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        Studio refreshes status indicators by calling <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET /api/v1/status</code> every two seconds. If the Core daemon becomes unresponsive, all indicators turn red and Studio displays a reconnecting banner.
                    </p>
                </section>

                {/* What's Next */}
                <section className="flex flex-col gap-4" id="whats-next">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">What's Next</h2>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        {[
                            { href: "/studio/editor", title: "Code Editor", desc: "Supported languages, keybindings, and theme configuration." },
                            { href: "/studio/forge", title: "Deployment — Forge", desc: "Full deployment flow, backend requirements, and package structure." },
                            { href: "/self-hosting", title: "Self-Hosting", desc: "Run Core without Studio on macOS, Linux, or Windows servers." },
                            { href: "/api-reference", title: "REST API Reference", desc: "Complete HTTP API documentation for Core." },
                        ].map((item, i) => (
                            <Link key={i} href={item.href} className="group flex flex-col gap-1.5 rounded-lg border border-border bg-[#111] hover:bg-[#141414] hover:border-primary/40 transition-all p-4">
                                <span className="font-medium text-foreground group-hover:text-primary transition-colors text-sm">{item.title} →</span>
                                <span className="text-xs text-muted-foreground leading-relaxed">{item.desc}</span>
                            </Link>
                        ))}
                    </div>
                </section>

                <PageNav href="/studio" />
            </div>
        </DocsLayout>
    );
}
