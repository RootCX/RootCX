"use client";

import { cn } from "@/lib/utils";
import { useEffect, useState } from "react";

export interface TocItem {
    id: string;
    title: string;
    level?: number;
}

interface TableOfContentsProps {
    items: TocItem[];
    className?: string;
}

export function TableOfContents({ items, className }: TableOfContentsProps) {
    const [activeId, setActiveId] = useState<string>(items[0]?.id ?? "");

    useEffect(() => {
        const observer = new IntersectionObserver(
            (entries) => {
                for (const entry of entries) {
                    if (entry.isIntersecting) {
                        setActiveId(entry.target.id);
                    }
                }
            },
            { rootMargin: "0px 0px -70% 0px", threshold: 0 }
        );

        items.forEach(({ id }) => {
            const el = document.getElementById(id);
            if (el) observer.observe(el);
        });

        return () => observer.disconnect();
    }, [items]);

    if (items.length === 0) return null;

    return (
        <div className={cn("hidden xl:block w-56 shrink-0 px-4", className)}>
            <div className="sticky top-[80px] flex flex-col gap-3">
                <h4 className="text-xs font-semibold uppercase tracking-widest text-muted-foreground/60">
                    On this page
                </h4>
                <nav className="flex flex-col gap-0 border-l border-border/50">
                    {items.map((item) => (
                        <a
                            key={item.id}
                            href={`#${item.id}`}
                            className={cn(
                                "border-l py-1.5 text-sm transition-colors",
                                item.level === 3 ? "pl-6" : "pl-3",
                                activeId === item.id
                                    ? "border-primary text-primary font-medium"
                                    : "border-transparent text-muted-foreground hover:text-foreground hover:border-border"
                            )}
                        >
                            {item.title}
                        </a>
                    ))}
                </nav>
            </div>
        </div>
    );
}
