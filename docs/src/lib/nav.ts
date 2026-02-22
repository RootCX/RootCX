export interface NavItem {
    title: string;
    href: string;
}

export interface NavSection {
    title: string;
    icon?: string;
    items: NavItem[];
}

export const navigation: NavSection[] = [
    {
        title: "1. Introduction",
        items: [
            { title: "What is RootCX?", href: "/" },
            { title: "Quickstart", href: "/quickstart" },
            { title: "How it Works", href: "/architecture" },
        ],
    },
    {
        title: "2. The Studio",
        items: [
            { title: "Studio Overview", href: "/studio" },
            { title: "Code Editor", href: "/studio/editor" },
            { title: "AI Forge", href: "/studio/forge" },
        ],
    },
    {
        title: "3. The Core",
        items: [
            { title: "Engine & Runtime", href: "/concepts/runtime" },
            { title: "App Manifest", href: "/concepts/manifest" },
            { title: "Data Contract", href: "/concepts/data-contract" },
            { title: "Roles & Permissions", href: "/concepts/permissions" },
        ],
    },
    {
        title: "4. Built-in Features",
        items: [
            { title: "Authentication", href: "/modules/authentication" },
            { title: "RBAC (Access Control)", href: "/modules/rbac" },
            { title: "Data Management", href: "/modules/data" },
            { title: "Backend & RPC", href: "/modules/backend" },
            { title: "Job Queue", href: "/modules/jobs" },
            { title: "Secret Vault", href: "/modules/secrets" },
            { title: "Real-time Logs", href: "/modules/logs" },
            { title: "Audit Trail", href: "/modules/audit" },
        ],
    },
    {
        title: "5. AI Integration",
        items: [
            { title: "AI Overview", href: "/ai" },
            { title: "LLM Providers", href: "/ai/providers" },
            { title: "Agents", href: "/ai/agents" },
            { title: "MCP Servers", href: "/ai/mcp" },
            { title: "Sessions & Commands", href: "/ai/sessions" },
        ],
    },
    {
        title: "6. Reference & Ops",
        items: [
            { title: "REST API", href: "/api-reference" },
            { title: "React SDK", href: "/sdk" },
            { title: "Self-Hosting", href: "/self-hosting" },
            { title: "Configuration", href: "/self-hosting/config" },
        ],
    },
];

export const allPages: NavItem[] = navigation.flatMap((s) => s.items);

export function getPrevNext(href: string): { prev: NavItem | null; next: NavItem | null } {
    const idx = allPages.findIndex((p) => p.href === href);
    return {
        prev: idx > 0 ? allPages[idx - 1] : null,
        next: idx < allPages.length - 1 ? allPages[idx + 1] : null,
    };
}
