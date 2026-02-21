import Link from "next/link";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { getPrevNext } from "@/lib/nav";

interface PageNavProps {
    href: string;
}

export function PageNav({ href }: PageNavProps) {
    const { prev, next } = getPrevNext(href);

    return (
        <div className="mt-16 flex items-center justify-between border-t border-border pt-8">
            {prev ? (
                <Link
                    href={prev.href}
                    className="group flex flex-col gap-1 text-sm transition-colors hover:text-foreground"
                >
                    <span className="flex items-center gap-1 text-xs text-muted-foreground">
                        <ChevronLeft className="h-3.5 w-3.5" />
                        Previous
                    </span>
                    <span className="font-medium text-muted-foreground group-hover:text-foreground transition-colors">
                        {prev.title}
                    </span>
                </Link>
            ) : (
                <div />
            )}

            {next ? (
                <Link
                    href={next.href}
                    className="group flex flex-col items-end gap-1 text-sm transition-colors hover:text-foreground"
                >
                    <span className="flex items-center gap-1 text-xs text-muted-foreground">
                        Next
                        <ChevronRight className="h-3.5 w-3.5" />
                    </span>
                    <span className="font-medium text-muted-foreground group-hover:text-foreground transition-colors">
                        {next.title}
                    </span>
                </Link>
            ) : (
                <div />
            )}
        </div>
    );
}
