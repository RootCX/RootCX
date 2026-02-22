import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
  { id: "outcomes", title: "Key Outcomes" },
  { id: "overview", title: "Overview" },
  { id: "getting-started", title: "Getting Started" },
  { id: "panels", title: "Panels" },
  { id: "command-palette", title: "Command Palette" },
  { id: "keyboard-shortcuts", title: "Keyboard Shortcuts" },
  { id: "status-bar", title: "Status Bar" },
];

export default function StudioPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/studio" className="hover:text-foreground transition-colors">Studio IDE</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Overview</span>
        </div>

        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Studio IDE</h1>
          <p className="text-lg text-muted-foreground leading-7">
            A local-first desktop IDE for building, deploying, and managing RootCX applications.
          </p>
        </div>

        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "All-in-one workspace: Write Backend code, design schemas, and manage deployments in a single application without juggling terminal tabs or separate tools.",
              "Automated embedded Core: The IDE automatically manages the embedded Core daemon lifecycle — starting on launch and shutting down on exit.",
              "Seamless feedback loop: Deploy new code to the embedded Core instantly and watch real-time console logs stream directly into your IDE."
            ].map((item, i) => (
              <li key={i} className="flex items-start gap-2">
                <span className="mt-2 flex-shrink-0 w-1.5 h-1.5 rounded-full bg-primary/60" />
                <span dangerouslySetInnerHTML={{ __html: item.replace(/^([^:]+:)/, '<strong>$1</strong>') }} />
              </li>
            ))}
          </ul>
        </section>

        <section className="flex flex-col gap-4" id="overview">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
          <p className="text-muted-foreground leading-7">
            RootCX Studio is a native desktop application built with <strong className="text-foreground font-medium">Tauri v2</strong> (Rust shell + React 19 frontend). It provides a fully integrated development environment purpose-built for RootCX applications, combining a{" "}
            <strong className="text-foreground font-medium">CodeMirror 6</strong> code editor, deployment tooling, live logs, and Core controls in a single window.
          </p>
          <p className="text-muted-foreground leading-7">
            Studio is the recommended way to develop RootCX applications locally. It automatically manages the Core daemon lifecycle, so you never need to start or stop PostgreSQL or the HTTP API manually. When Studio opens, the daemon starts. When Studio closes, the daemon stops.
          </p>
          <Callout variant="info" title="Studio bundles the Core binary">
            Studio ships with the RootCX Core binary embedded as a Tauri sidecar. You do not need to install Core separately. The correct binary for your platform is bundled automatically at build time.
          </Callout>
        </section>

        <section className="flex flex-col gap-4" id="getting-started">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Getting Started</h2>
          <p className="text-muted-foreground leading-7">
            Download the latest Studio release for your platform from the GitHub Releases page.
          </p>
          <div className="rounded-lg border border-border overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-4 py-3 text-left font-medium text-foreground">Platform</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Format</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["macOS (Apple Silicon)", ".dmg"],
                  ["macOS (Intel)", ".dmg"],
                  ["Linux x86_64", ".AppImage / .deb"],
                  ["Windows x86_64", ".msi"],
                ].map(([platform, fmt], i) => (
                  <tr key={i} className="border-b border-border last:border-0">
                    <td className="px-4 py-3 text-foreground text-xs">{platform}</td>
                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{fmt}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="text-muted-foreground leading-7">
            On first launch, Studio starts the bundled Core daemon, polls{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET /api/v1/status</code>{" "}
            every three seconds until healthy, then shows green indicators in the status bar.
          </p>
          <Callout variant="tip" title="First-time setup">
            On macOS, you may need to right-click the app and choose Open the first time to bypass Gatekeeper.
          </Callout>
        </section>

        <section className="flex flex-col gap-4" id="panels">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Panels</h2>
          <p className="text-muted-foreground leading-7">
            Studio is organized into a set of panels in a fixed layout. Each panel handles a specific workflow.
          </p>
          <div className="rounded-lg border border-border overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-4 py-3 text-left font-medium text-foreground">Panel</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Position</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Purpose</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["File Explorer", "Left sidebar", "Browse and open workspace files"],
                  ["Code Editor", "Center", "Edit TypeScript, JSON, and other files (CodeMirror 6)"],
                  ["Forge", "Right sidebar", "Deploy backends and manage lifecycle"],
                  ["Console", "Bottom drawer", "Live log stream from running backends (SSE)"],
                  ["Command Palette", "Overlay", "Keyboard-driven access to all commands"],
                  ["Status Bar", "Bottom strip", "PostgreSQL, Core, and Backend health indicators"],
                ].map(([panel, pos, purpose], i) => (
                  <tr key={i} className="border-b border-border last:border-0">
                    <td className="px-4 py-3 font-medium text-foreground text-xs">{panel}</td>
                    <td className="px-4 py-3 text-muted-foreground text-xs">{pos}</td>
                    <td className="px-4 py-3 text-muted-foreground text-xs">{purpose}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="text-muted-foreground leading-7">
            The default workspace path is{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/RootCX/apps/{"{appId}"}/</code>.
            You can open any folder as a workspace using the command palette.
          </p>
        </section>

        <section className="flex flex-col gap-4" id="command-palette">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Command Palette</h2>
          <p className="text-muted-foreground leading-7">
            Press <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘K</code> (macOS)
            or <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Ctrl+K</code> (Linux/Windows)
            to open the command palette. Type to filter, arrow keys to navigate, Enter to execute.
          </p>
          <div className="rounded-lg border border-border overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-4 py-3 text-left font-medium text-foreground">Command</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Description</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["Open File…", "Quick-open any file in the workspace by name"],
                  ["Deploy Backend", "Package and deploy the current workspace"],
                  ["Start Backend", "Start the backend without re-deploying"],
                  ["Stop Backend", "Gracefully stop the running backend"],
                  ["Clear Console", "Wipe the console log buffer"],
                  ["Open Workspace…", "Choose a different folder as the workspace"],
                  ["Install App from Manifest…", "POST a manifest to Core to install or update an app"],
                ].map(([cmd, desc], i) => (
                  <tr key={i} className="border-b border-border last:border-0">
                    <td className="px-4 py-3 font-mono text-xs text-foreground">{cmd}</td>
                    <td className="px-4 py-3 text-muted-foreground text-xs">{desc}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        <section className="flex flex-col gap-4" id="keyboard-shortcuts">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Keyboard Shortcuts</h2>
          <p className="text-muted-foreground leading-7">
            On Linux and Windows, substitute{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Ctrl</code> for{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">⌘</code>.
          </p>
          <div className="rounded-lg border border-border overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-4 py-3 text-left font-medium text-foreground">Shortcut</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Action</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["⌘K", "Open command palette"],
                  ["⌘S", "Save current file"],
                  ["⌘P", "Quick-open file"],
                  ["⌘`", "Toggle console panel"],
                  ["⌘F", "Find in current file"],
                  ["⌘/", "Toggle line comment"],
                  ["⌘⇧P", "Deploy backend"],
                ].map(([shortcut, action], i) => (
                  <tr key={i} className="border-b border-border last:border-0">
                    <td className="px-4 py-3">
                      <kbd className="rounded border border-border bg-muted/30 px-2 py-0.5 font-mono text-xs text-foreground">{shortcut}</kbd>
                    </td>
                    <td className="px-4 py-3 text-muted-foreground text-xs">{action}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        <section className="flex flex-col gap-4" id="status-bar">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Status Bar</h2>
          <p className="text-muted-foreground leading-7">
            The status bar at the bottom of Studio shows real-time health indicators for three subsystems:
          </p>
          <div className="flex flex-col gap-2">
            {[
              ["PostgreSQL", "Embedded PostgreSQL is running on port 5480"],
              ["Core", "Core HTTP API is responding on port 9100"],
              ["Backend", "Green = running, grey = stopped, red = crashed"],
            ].map(([label, desc]) => (
              <div key={label} className="flex items-start gap-3 rounded-lg border border-border/50 bg-card px-4 py-3">
                <span className="mt-1.5 flex-shrink-0 w-2 h-2 rounded-full bg-green-500" />
                <div>
                  <p className="text-sm font-medium text-foreground">{label}</p>
                  <p className="text-xs text-muted-foreground mt-0.5">{desc}</p>
                </div>
              </div>
            ))}
          </div>
          <p className="text-muted-foreground leading-7">
            Studio polls <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET /api/v1/status</code> every
            three seconds. If Core becomes unresponsive, all indicators turn red and Studio displays a reconnecting banner.
          </p>
        </section>

        <section className="flex flex-col gap-4">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">{"What's Next"}</h2>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {[
              { href: "/studio/editor", title: "Code Editor", desc: "Supported languages, keybindings, and theme configuration." },
              { href: "/studio/forge", title: "Deployment — Forge", desc: "Full deployment flow and backend requirements." },
              { href: "/self-hosting", title: "Self-Hosting", desc: "Run Core without Studio on macOS, Linux, or Windows." },
              { href: "/api-reference", title: "REST API Reference", desc: "Complete HTTP API documentation for Core." },
            ].map((item, i) => (
              <Link key={i} href={item.href} className="group flex flex-col gap-1.5 rounded-lg border border-border hover:border-primary/40 transition-all p-4">
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
