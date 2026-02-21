import { Search, Github, Layers } from "lucide-react";
import Link from "next/link";

export function Topbar() {
    return (
        <header className="sticky top-0 z-50 flex h-14 items-center justify-between border-b border-border bg-[#0a0a0a]/90 px-6 backdrop-blur-md shrink-0">
            <div className="flex items-center gap-6">
                <Link href="/" className="flex items-center gap-2 font-bold text-foreground transition-opacity hover:opacity-80">
                    <Layers className="h-5 w-5 text-primary" />
                    <span className="tracking-tight">RootCX</span>
                    <span className="ml-1 hidden rounded-full border border-border px-2 py-0.5 text-[10px] font-medium text-muted-foreground sm:inline-flex">
                        Docs
                    </span>
                </Link>
                <nav className="hidden md:flex gap-1">
                    <Link href="/" className="rounded-md px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground">
                        Guides
                    </Link>
                    <Link href="/api-reference" className="rounded-md px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground">
                        API
                    </Link>
                    <Link href="/sdk" className="rounded-md px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground">
                        SDK
                    </Link>
                    <Link href="/self-hosting" className="rounded-md px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground">
                        Self-Hosting
                    </Link>
                </nav>
            </div>

            <div className="flex items-center gap-3">
                <div className="group relative flex cursor-pointer items-center rounded-md border border-border bg-[#141414] px-3 py-1.5 text-sm text-muted-foreground transition-colors hover:border-border/80 hover:text-foreground md:w-56 lg:w-72">
                    <Search className="mr-2 h-3.5 w-3.5 shrink-0" />
                    <span className="flex-1 text-left hidden sm:inline-block text-xs">Search documentation...</span>
                    <kbd className="pointer-events-none ml-2 hidden h-5 select-none items-center gap-1 rounded border border-border bg-muted/50 px-1.5 font-mono text-[10px] font-medium opacity-100 sm:flex">
                        <span className="text-xs">⌘</span>K
                    </kbd>
                </div>

                <Link
                    href="https://github.com/rootcx/rootcx"
                    target="_blank"
                    rel="noreferrer"
                    className="flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
                >
                    <Github className="h-4 w-4" />
                    <span className="sr-only">GitHub</span>
                </Link>
            </div>
        </header>
    );
}
