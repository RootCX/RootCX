import { Sidebar } from "./Sidebar";
import { Topbar } from "./Topbar";
import { TableOfContents, TocItem } from "./TableOfContents";

interface DocsLayoutProps {
    children: React.ReactNode;
    toc?: TocItem[];
}

export function DocsLayout({ children, toc = [] }: DocsLayoutProps) {
    return (
        <div className="flex h-screen w-full flex-col overflow-hidden bg-background">
            <Topbar />
            <div className="flex flex-1 overflow-hidden">
                <Sidebar className="hidden w-60 md:flex shrink-0" />
                <main className="flex-1 overflow-y-auto w-full pb-32">
                    <div className="mx-auto flex max-w-[1400px] justify-center px-4 md:px-8 py-8 md:py-12">
                        <article className="w-full max-w-[760px] flex-1 min-w-0 xl:mr-8">
                            {children}
                        </article>
                        <TableOfContents items={toc} />
                    </div>
                </main>
            </div>
        </div>
    );
}
