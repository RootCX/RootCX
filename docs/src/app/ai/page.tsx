import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight, ArrowRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "what-you-can-build", title: "What you can build" },
    { id: "how-to-work-with-forge", title: "How to work with Forge" },
    { id: "a-real-example", title: "A real example" },
    { id: "next-steps", title: "Next steps" },
];

export default function AIOverviewPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Building with AI</span>
                </div>

                <header className="flex flex-col gap-4" id="overview">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Building with AI</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        RootCX gives AI a structured surface to work with — a manifest, a known API shape, a typed worker contract. Forge and the AI turn that structure into running software.
                    </p>
                </header>

                <section className="flex flex-col gap-4">
                    <p className="text-muted-foreground leading-7">
                        Most platforms are blank canvases. AI on a blank canvas produces inconsistent output that needs heavy review and correction. RootCX is the opposite: your data model is declared in a manifest, your business logic lives in typed workers with a fixed interface, RBAC rules are explicit, secrets have names. The AI can read all of this, understand constraints before writing a single line, and produce code that actually fits.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        <strong className="text-foreground font-medium">AI Forge</strong> is the Studio panel that connects to <strong className="text-foreground font-medium">OpenCode</strong> — an open-source AI coding engine embedded in Studio. You bring your own LLM (Anthropic, OpenAI, GitHub Copilot, or local). Forge handles the rest: reading your project, orchestrating multi-agent workflows, proposing file changes for your approval, and streaming results in real time.
                    </p>
                </section>

                <section className="flex flex-col gap-6" id="what-you-can-build">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">What you can build</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX is purpose-built for custom internal software — the kind of app that no SaaS product covers exactly right, and that your team has been living without or hacking around in spreadsheets.
                    </p>
                    <div className="flex flex-col gap-4">

                        <div className="rounded-xl border border-border bg-[#111] p-5 flex flex-col gap-3">
                            <h3 className="font-semibold text-foreground">CRM & sales pipeline</h3>
                            <p className="text-sm text-muted-foreground leading-relaxed">
                                Tables for contacts, companies, deals, pipeline stages, activities, and notes. Row-level access so sales reps see only their accounts; managers see the full team. Workers handle deal stage transitions — sending Slack notifications when a deal moves to <em>Proposal</em>, logging a job to generate a PDF quote, calling an external enrichment API when a new contact is created.
                            </p>
                        </div>

                        <div className="rounded-xl border border-border bg-[#111] p-5 flex flex-col gap-3">
                            <h3 className="font-semibold text-foreground">Billing & subscription management</h3>
                            <p className="text-sm text-muted-foreground leading-relaxed">
                                Tables for plans, subscriptions, invoices, and usage events. A worker that receives Stripe webhooks via RPC and writes the correct records — updating subscription status on <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">customer.subscription.updated</code>, creating an invoice row on <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">invoice.paid</code>, sending an overdue email via a background job. RBAC that lets customers see only their own invoices.
                            </p>
                        </div>

                        <div className="rounded-xl border border-border bg-[#111] p-5 flex flex-col gap-3">
                            <h3 className="font-semibold text-foreground">Fleet & field operations</h3>
                            <p className="text-sm text-muted-foreground leading-relaxed">
                                Vehicles, drivers, assignments, maintenance schedules, fuel logs. Technicians see their own work orders and nothing else. Dispatchers assign jobs, see all vehicles and their status. A job runs nightly to flag vehicles where maintenance is overdue. A worker RPC triggers a driver assignment and notifies them via SMS through the Twilio API using a secret stored in the vault.
                            </p>
                        </div>

                        <div className="rounded-xl border border-border bg-[#111] p-5 flex flex-col gap-3">
                            <h3 className="font-semibold text-foreground">Document approval workflows</h3>
                            <p className="text-sm text-muted-foreground leading-relaxed">
                                Documents, versions, approval chains with ordered steps. Submitters create documents and see only their own submissions. Approvers see documents at their step. Each approval or rejection triggers a worker job — advancing to the next step, notifying the next approver, or closing the document and emailing the submitter. Full audit trail of every status change, courtesy of PostgreSQL triggers.
                            </p>
                        </div>

                        <div className="rounded-xl border border-border bg-[#111] p-5 flex flex-col gap-3">
                            <h3 className="font-semibold text-foreground">Industry-specific data tools</h3>
                            <p className="text-sm text-muted-foreground leading-relaxed">
                                The manifest supports any relational model. A construction company manages projects, subcontractors, cost codes, and punch lists. A clinical team tracks patients, protocols, dosing events, and adverse reactions with strict per-researcher access. A logistics company models shipments, legs, carriers, and real-time status updates from carrier APIs polled by a background job. The pattern is always the same: define the schema, write the access rules, deploy the worker logic.
                            </p>
                        </div>

                        <div className="rounded-xl border border-border bg-[#111] p-5 flex flex-col gap-3">
                            <h3 className="font-semibold text-foreground">AI agent backends</h3>
                            <p className="text-sm text-muted-foreground leading-relaxed">
                                Workers can themselves call LLMs. A customer support worker receives a ticket via RPC, calls the OpenAI API to classify and draft a reply, writes the result back to the database, and enqueues a human review job if confidence is low. The job queue handles retry logic and volume spikes. Secrets store the OpenAI key. The audit log captures every AI-generated action for compliance review.
                            </p>
                        </div>

                    </div>
                </section>

                <section className="flex flex-col gap-4" id="how-to-work-with-forge">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">How to work with Forge</h2>
                    <p className="text-muted-foreground leading-7">
                        Forge works best when you treat it as a collaborator that reads your project before acting. The typical flow:
                    </p>
                    <div className="flex flex-col gap-0 rounded-xl border border-border overflow-hidden">
                        {[
                            {
                                step: "01",
                                title: "Describe the goal precisely",
                                desc: "Not \"build a CRM\" — that's too vague. \"Add a contacts table with first_name, last_name, email (unique), company_id (foreign key to companies), and owner_id (foreign key to users). Only the owner can update or delete a contact. Admins can see all contacts.\" The more specific you are about schema, access rules, and behavior, the less back-and-forth you need.",
                            },
                            {
                                step: "02",
                                title: "Review the plan before the build",
                                desc: "Forge will propose what it intends to do before writing any files. Read it. Push back on any step you disagree with. It's much cheaper to redirect the plan than to revert committed changes.",
                            },
                            {
                                step: "03",
                                title: "Approve file diffs one by one",
                                desc: "Every write to manifest.json or a worker file requires your explicit approval. Review the diff. Pay particular attention to manifest.json — schema changes are applied to PostgreSQL when you install the app.",
                            },
                            {
                                step: "04",
                                title: "Iterate in the same session",
                                desc: "Forge remembers what was built earlier in the session. \"Add a job that sends a weekly digest email to each owner\" builds on the contacts table it just created. You don't need to re-explain the schema.",
                            },
                        ].map((s, i) => (
                            <div key={i} className="flex items-start gap-4 p-5 border-b border-border last:border-0 hover:bg-white/[0.02] transition-colors">
                                <span className="mt-0.5 font-mono text-xs font-bold text-primary/50 shrink-0 w-6">{s.step}</span>
                                <div>
                                    <p className="font-medium text-foreground text-sm">{s.title}</p>
                                    <p className="text-xs text-muted-foreground leading-relaxed mt-0.5">{s.desc}</p>
                                </div>
                            </div>
                        ))}
                    </div>
                    <Callout variant="tip" title="Give context with instructions">
                        Create an <code>AGENTS.md</code> (or any Markdown file) at the root of your project with notes about your architecture, conventions, and team decisions. OpenCode reads these instruction files automatically — they show up in the Settings panel under <strong>Instructions</strong>. Use them to avoid repeating yourself across sessions.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="a-real-example">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">A real example</h2>
                    <p className="text-muted-foreground leading-7">
                        Building a leave request system from scratch. One conversation, manifest and worker produced end-to-end.
                    </p>
                    <div className="rounded-xl border border-border bg-[#0d0d0d] divide-y divide-border">
                        {[
                            {
                                role: "You",
                                color: "text-blue-300",
                                text: "Add a leave management system. Tables: employees (user_id FK, department, manager_id FK to employees), leave_types (name, max_days_per_year), leave_requests (employee_id FK, leave_type_id FK, start_date, end_date, status enum open/approved/rejected, approved_by FK to employees, notes). Employees can create requests and see only their own. Managers can see and update requests for their direct reports. HR role can see and update everything.",
                            },
                            {
                                role: "Forge",
                                color: "text-primary",
                                text: "Plan: (1) Add three tables to manifest.json with the described columns and FKs. (2) Add RBAC: employee gets create + read-own on leave_requests; manager gets read + update where approved_by is their employee_id subtree; hr gets full access. (3) Scaffold a worker with a submitLeaveRequest RPC that validates remaining balance and a notifyManager job. Should I proceed?",
                            },
                            {
                                role: "You",
                                color: "text-blue-300",
                                text: "Yes. Also add a leaveBalance view that calculates used days per employee per year.",
                            },
                            {
                                role: "Forge",
                                color: "text-primary",
                                text: "Writing manifest.json... [diff proposed] Writing worker/index.ts... [diff proposed] Writing worker/balance.ts for the view logic...",
                            },
                        ].map((turn, i) => (
                            <div key={i} className="px-5 py-4 flex flex-col gap-1">
                                <span className={`text-[10px] font-bold uppercase tracking-widest ${turn.color}`}>{turn.role}</span>
                                <p className="text-sm text-muted-foreground leading-relaxed">{turn.text}</p>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        After approving the diffs, you install the manifest from Studio — PostgreSQL tables are created, RBAC triggers applied. Deploy the worker. Done. Total time from empty project to running system: one focused session.
                    </p>
                    <CodeBlock
                        language="typescript"
                        filename="worker/index.ts (produced by Forge)"
                        code={`import type { Caller } from "@rootcx/worker";

export async function handleRpc(
  method: string,
  params: Record<string, unknown>,
  caller: Caller
): Promise<unknown> {
  if (method === "submitLeaveRequest") {
    const { leaveTypeId, startDate, endDate, notes } = params as {
      leaveTypeId: string;
      startDate: string;
      endDate: string;
      notes?: string;
    };

    // Validate balance before inserting
    const used = await db.query(
      \`SELECT COALESCE(SUM(end_date - start_date + 1), 0)
       FROM leave_requests
       WHERE employee_id = $1
         AND leave_type_id = $2
         AND EXTRACT(year FROM start_date) = EXTRACT(year FROM CURRENT_DATE)
         AND status != 'rejected'\`,
      [caller.userId, leaveTypeId]
    );

    const max = await db.queryOne(
      "SELECT max_days_per_year FROM leave_types WHERE id = $1",
      [leaveTypeId]
    );

    const requested = daysBetween(startDate, endDate);
    if (Number(used.rows[0][0]) + requested > max.max_days_per_year) {
      throw new Error("Insufficient leave balance");
    }

    const request = await db.insert("leave_requests", {
      employee_id: caller.userId,
      leave_type_id: leaveTypeId,
      start_date: startDate,
      end_date: endDate,
      notes,
      status: "open",
    });

    await jobs.enqueue("notifyManager", { requestId: request.id });
    return request;
  }

  throw new Error(\`Unknown method: \${method}\`);
}`}
                    />
                </section>

                <section className="flex flex-col gap-4" id="next-steps">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Next steps</h2>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        {[
                            { href: "/ai/providers", title: "LLM Providers", desc: "Connect Anthropic, OpenAI, GitHub Copilot, or a local model." },
                            { href: "/ai/agents", title: "Agents", desc: "How plan, build, explore, and general agents work." },
                            { href: "/ai/mcp", title: "MCP Servers", desc: "Extend AI tools with Model Context Protocol servers." },
                            { href: "/ai/sessions", title: "Sessions & Commands", desc: "Persistent sessions and reusable command templates." },
                        ].map((l, i) => (
                            <Link
                                key={i}
                                href={l.href}
                                className="group flex flex-col gap-1.5 rounded-xl border border-border bg-[#111] p-4 hover:bg-[#141414] hover:border-primary/40 transition-all"
                            >
                                <div className="flex items-center justify-between">
                                    <span className="font-semibold text-foreground text-sm group-hover:text-primary transition-colors">{l.title}</span>
                                    <ArrowRight className="h-3.5 w-3.5 text-muted-foreground/30 group-hover:text-primary/60 transition-colors" />
                                </div>
                                <span className="text-xs text-muted-foreground leading-relaxed">{l.desc}</span>
                            </Link>
                        ))}
                    </div>
                </section>

                <PageNav href="/ai" />
            </div>
        </DocsLayout>
    );
}
