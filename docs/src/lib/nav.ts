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
        title: "Getting Started",
        items: [
            { title: "What is RootCX?", href: "/" },
            { title: "Quick Start", href: "/quickstart" },
            { title: "Architecture", href: "/architecture" },
        ],
    },
    {
        title: "Core Concepts",
        items: [
            { title: "App Manifest", href: "/concepts/manifest" },
            { title: "Data Contract", href: "/concepts/data-contract" },
            { title: "Engine & Runtime", href: "/concepts/runtime" },
            { title: "Roles & Permissions", href: "/concepts/permissions" },
        ],
    },
    {
        title: "Native Modules",
        items: [
            { title: "Data Management", href: "/modules/data" },
            { title: "Authentication", href: "/modules/authentication" },
            { title: "RBAC", href: "/modules/rbac" },
            { title: "Audit Logs", href: "/modules/audit" },
            { title: "Secret Vault", href: "/modules/secrets" },
            { title: "Job Queue", href: "/modules/jobs" },
            { title: "Backend & RPC", href: "/modules/backend" },
            { title: "Real-time Logs", href: "/modules/logs" },
        ],
    },
    {
        title: "Studio IDE",
        items: [
            { title: "Overview", href: "/studio" },
            { title: "Code Editor", href: "/studio/editor" },
            { title: "AI Forge", href: "/studio/forge" },
        ],
    },
    {
        title: "Building with AI",
        items: [
            { title: "Overview", href: "/ai" },
            { title: "LLM Providers", href: "/ai/providers" },
            { title: "Agents", href: "/ai/agents" },
            { title: "MCP Servers", href: "/ai/mcp" },
            { title: "Sessions & Commands", href: "/ai/sessions" },
        ],
    },
    {
        title: "API Reference",
        items: [
            { title: "REST API", href: "/api-reference" },
            { title: "React SDK", href: "/sdk" },
        ],
    },
    {
        title: "Self-Hosting",
        items: [
            { title: "Overview", href: "/self-hosting" },
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
