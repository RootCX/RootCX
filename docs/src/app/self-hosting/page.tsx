import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "platforms", title: "Platforms" },
    { id: "macos", title: "macOS" },
    { id: "linux", title: "Linux" },
    { id: "windows", title: "Windows" },
    { id: "running-as-service", title: "Running as a service" },
    { id: "ports", title: "Ports" },
    { id: "data-directory", title: "Data directory" },
    { id: "upgrades", title: "Upgrades" },
];

export default function SelfHostingPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Self-Hosting</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Self-Hosting</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Run RootCX entirely on your own infrastructure — no external dependencies, no cloud required.
                    </p>
                </header>

                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX is designed to be entirely self-hosted. The Core daemon ships as a single statically-linked binary that embeds everything it needs — PostgreSQL 18.1, the Bun runtime, and all system dependencies. You do not need Docker, a database server, or any external services.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        All runtime data is stored locally under the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/RootCX/</code> directory. Nothing leaves your machine unless your own application code makes outbound network requests.
                    </p>
                    <Callout variant="tip" title="Fully offline capable">
                        Once installed, RootCX runs entirely offline. It requires no internet connection for operation — only for the initial binary download.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="platforms">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Platforms</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX ships pre-built binaries for all major platforms:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Platform</th>
                                    <th className="px-4 py-3 text-left font-semibold">Architecture</th>
                                    <th className="px-4 py-3 text-left font-semibold">Binary name</th>
                                    <th className="px-4 py-3 text-left font-semibold">Studio format</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["macOS", "Apple Silicon (arm64)", "rootcx-core-darwin-arm64", ".dmg"],
                                    ["macOS", "Intel (x86_64)", "rootcx-core-darwin-x86_64", ".dmg"],
                                    ["macOS", "Universal", "rootcx-core-darwin-universal", ".dmg"],
                                    ["Linux", "x86_64", "rootcx-core-linux-x86_64", ".AppImage / .tar.gz"],
                                    ["Linux", "ARM64", "rootcx-core-linux-arm64", ".AppImage / .tar.gz"],
                                    ["Windows", "x86_64", "rootcx-core-windows-x86_64.exe", ".msi / .exe"],
                                ].map(([platform, arch, binary, studio], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0 hover:bg-white/[0.02]">
                                        <td className="px-4 py-3 text-sm text-foreground">{platform}</td>
                                        <td className="px-4 py-3 text-sm text-muted-foreground">{arch}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{binary}</td>
                                        <td className="px-4 py-3 text-xs text-muted-foreground">{studio}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <p className="text-muted-foreground leading-7">
                        Download from the <strong className="text-foreground font-medium">GitHub Releases</strong> page. Both the standalone Core binary and the full Studio desktop app are available for each platform.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="macos">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">macOS</h2>
                    <p className="text-muted-foreground leading-7">
                        For the full Studio experience, download and open the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.dmg</code> file. To run the headless daemon only:
                    </p>
                    <CodeBlock language="bash" code={`# Apple Silicon
curl -L https://github.com/rootcx/rootcx/releases/latest/download/rootcx-core-darwin-arm64 \\
  -o rootcx-core
chmod +x rootcx-core

# Start the daemon
./rootcx-core start

# Verify
curl http://localhost:9100/health`} />
                    <p className="text-muted-foreground leading-7">
                        The binary is self-contained — no Homebrew, no pkg install, no system dependencies.
                    </p>
                    <Callout variant="info" title="Gatekeeper">
                        On first run, macOS may block the binary because it is not notarized. Right-click → Open, or run <code>xattr -d com.apple.quarantine ./rootcx-core</code> to allow it.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="linux">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Linux</h2>
                    <CodeBlock language="bash" code={`# x86_64
curl -L https://github.com/rootcx/rootcx/releases/latest/download/rootcx-core-linux-x86_64 \\
  -o rootcx-core
chmod +x rootcx-core

# Move to a system-wide location (optional)
sudo mv rootcx-core /usr/local/bin/rootcx-core

# Start
rootcx-core start`} />
                    <p className="text-muted-foreground leading-7">
                        The binary links statically against musl libc on Linux and requires no system libraries.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="windows">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Windows</h2>
                    <CodeBlock language="powershell" code={`# Download via PowerShell
Invoke-WebRequest -Uri "https://github.com/rootcx/rootcx/releases/latest/download/rootcx-core-windows-x86_64.exe" -OutFile "rootcx-core.exe"

# Start
.\rootcx-core.exe start`} />
                    <p className="text-muted-foreground leading-7">
                        On Windows, data is stored in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">%USERPROFILE%\RootCX\</code> by default.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="running-as-service">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Running as a service</h2>
                    <p className="text-muted-foreground leading-7">
                        For production deployments, register RootCX as a system service to ensure it starts on boot and restarts on failure.
                    </p>

                    <h3 className="text-lg font-semibold text-foreground mt-2">macOS — launchd</h3>
                    <CodeBlock language="xml" filename="~/Library/LaunchAgents/cx.root.core.plist" code={`<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>cx.root.core</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/rootcx-core</string>
    <string>start</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>ROOTCX_AUTH</key>
    <string>required</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/Users/you/RootCX/logs/runtime.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/you/RootCX/logs/runtime.log</string>
</dict>
</plist>`} />
                    <CodeBlock language="bash" code={`# Load and start
launchctl load ~/Library/LaunchAgents/cx.root.core.plist
launchctl start cx.root.core`} />

                    <h3 className="text-lg font-semibold text-foreground mt-4">Linux — systemd</h3>
                    <CodeBlock language="ini" filename="/etc/systemd/system/rootcx.service" code={`[Unit]
Description=RootCX Core Daemon
After=network.target

[Service]
Type=simple
User=ubuntu
ExecStart=/usr/local/bin/rootcx-core start
Restart=on-failure
RestartSec=5s
Environment=ROOTCX_AUTH=required
EnvironmentFile=/opt/rootcx/.env

StandardOutput=append:/var/log/rootcx/runtime.log
StandardError=append:/var/log/rootcx/runtime.log

[Install]
WantedBy=multi-user.target`} />
                    <CodeBlock language="bash" code={`sudo systemctl daemon-reload
sudo systemctl enable rootcx
sudo systemctl start rootcx
sudo systemctl status rootcx`} />
                </section>

                <section className="flex flex-col gap-4" id="ports">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Ports</h2>
                    <p className="text-muted-foreground leading-7">
                        By default, RootCX binds both ports to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">127.0.0.1</code> (localhost only). No ports are exposed to the network without explicit configuration.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Port</th>
                                    <th className="px-4 py-3 text-left font-semibold">Service</th>
                                    <th className="px-4 py-3 text-left font-semibold">Override</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["9100", "Core HTTP API", "ROOTCX_PORT"],
                                    ["5480", "Embedded PostgreSQL", "ROOTCX_PG_PORT"],
                                ].map(([port, service, override], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{port}</td>
                                        <td className="px-4 py-3 text-sm text-muted-foreground">{service}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-muted-foreground">{override}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <p className="text-muted-foreground leading-7">
                        To expose the API externally (e.g., behind a reverse proxy), use nginx or Caddy to proxy requests to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">localhost:9100</code> and add TLS termination there.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="data-directory">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Data directory</h2>
                    <p className="text-muted-foreground leading-7">
                        All persistent data lives under <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/RootCX/</code> (or <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">%USERPROFILE%\RootCX\</code> on Windows). Override with:
                    </p>
                    <CodeBlock language="bash" code={`ROOTCX_DATA_DIR=/opt/rootcx/data ./rootcx-core start`} />
                    <p className="text-muted-foreground leading-7">
                        To back up your entire RootCX instance, stop the daemon and copy the data directory:
                    </p>
                    <CodeBlock language="bash" code={`# Stop the daemon
systemctl stop rootcx   # or: launchctl stop cx.root.core

# Backup
tar -czf rootcx-backup-$(date +%Y%m%d).tar.gz ~/RootCX/

# Restart
systemctl start rootcx`} />
                    <Callout variant="warning" title="Stop before backup">
                        Always stop the daemon before backing up the data directory. PostgreSQL may have unflushed pages that could corrupt the backup if copied while running.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="upgrades">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Upgrades</h2>
                    <p className="text-muted-foreground leading-7">
                        Upgrading RootCX is straightforward — the database and configuration files are preserved across binary upgrades:
                    </p>
                    <CodeBlock language="bash" code={`# 1. Stop the current daemon
systemctl stop rootcx

# 2. Download the new binary
curl -L https://github.com/rootcx/rootcx/releases/latest/download/rootcx-core-linux-x86_64 \\
  -o /usr/local/bin/rootcx-core.new
chmod +x /usr/local/bin/rootcx-core.new

# 3. Replace the binary atomically
mv /usr/local/bin/rootcx-core.new /usr/local/bin/rootcx-core

# 4. Start the new version
systemctl start rootcx

# 5. Verify
curl http://localhost:9100/health`} />
                    <p className="text-muted-foreground leading-7">
                        On startup, the new binary will run any necessary schema migrations on the system database automatically. Your application data and configuration are untouched.
                    </p>
                    <Callout variant="info" title="Downgrade safety">
                        Keep a copy of the previous binary before upgrading. If something goes wrong, stop the daemon, restore the old binary, and restart.
                    </Callout>
                </section>

                <PageNav href="/self-hosting" />
            </div>
        </DocsLayout>
    );
}
