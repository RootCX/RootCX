"use client";

import { cn } from "@/lib/utils";
import { navigation } from "@/lib/nav";
import { usePathname } from "next/navigation";
import Link from "next/link";
import {
    Play,
    Blocks,
    Package,
    Layout,
    Code,
    Server,
} from "lucide-react";

const sectionIcons: Record<string, React.ReactNode> = {
    "Getting Started": <Play className="h-4 w-4" />,
    "Core Concepts": <Blocks className="h-4 w-4" />,
    "Native Modules": <Package className="h-4 w-4" />,
    "Studio IDE": <Layout className="h-4 w-4" />,
    "API Reference": <Code className="h-4 w-4" />,
    "Self-Hosting": <Server className="h-4 w-4" />,
};

export function Sidebar({ className }: { className?: string }) {
    const pathname = usePathname();

    return (
        <aside className={cn("flex flex-col bg-[#111111] border-r border-border h-full overflow-y-auto", className)}>
            <div className="p-4 px-6 pt-6 pb-16">
                <nav className="flex flex-col gap-6">
                    {navigation.map((section, i) => (
                        <div key={i} className="flex flex-col gap-2">
                            <div className="flex items-center gap-2 text-sm font-semibold text-foreground">
                                {sectionIcons[section.title]}
                                <span>{section.title}</span>
                            </div>
                            <div className="flex flex-col ml-[9px] border-l border-border/50 pl-4 py-1 flex-1 gap-0.5">
                                {section.items.map((item, j) => {
                                    const isActive = pathname === item.href;
                                    return (
                                        <Link
                                            key={j}
                                            href={item.href}
                                            className={cn(
                                                "text-sm py-1.5 px-2 rounded-md transition-colors duration-150",
                                                isActive
                                                    ? "text-primary font-medium bg-primary/10"
                                                    : "text-muted-foreground hover:text-foreground hover:bg-white/5"
                                            )}
                                        >
                                            {item.title}
                                        </Link>
                                    );
                                })}
                            </div>
                        </div>
                    ))}
                </nav>
            </div>
        </aside>
    );
}
