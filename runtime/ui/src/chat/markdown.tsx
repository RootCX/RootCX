import type { HTMLAttributes, OlHTMLAttributes, AnchorHTMLAttributes, ComponentType } from "react";
import ReactMarkdown from "react-markdown";
import { cn } from "../lib/utils";

const heading = (p: HTMLAttributes<HTMLHeadingElement>) => (
  <h3 className="mt-4 mb-2 text-[15px] font-semibold text-foreground" {...p} />
);

const defaultComponents: Record<string, ComponentType> = {
  p: (p: HTMLAttributes<HTMLParagraphElement>) => <p className="my-2 first:mt-0 last:mb-0 leading-relaxed" {...p} />,
  strong: (p: HTMLAttributes<HTMLElement>) => <strong className="font-semibold text-foreground" {...p} />,
  ul: (p: HTMLAttributes<HTMLUListElement>) => <ul className="my-2 list-disc pl-5 marker:text-muted-foreground/50" {...p} />,
  ol: (p: OlHTMLAttributes<HTMLOListElement>) => <ol className="my-2 list-decimal pl-5" {...p} />,
  li: (p: HTMLAttributes<HTMLLIElement>) => <li className="my-1 leading-relaxed" {...p} />,
  h1: heading, h2: heading, h3: heading,
  code: ({ className, children, ...rest }: HTMLAttributes<HTMLElement>) =>
    className
      ? <pre className="my-3 overflow-x-auto rounded-lg bg-muted/50 px-4 py-3 font-mono text-[13px] leading-relaxed text-foreground/90"><code {...rest}>{children}</code></pre>
      : <code className="rounded-md bg-muted px-1.5 py-0.5 font-mono text-[13px] text-foreground/90" {...rest}>{children}</code>,
  pre: ({ children }: HTMLAttributes<HTMLPreElement>) => <>{children}</>,
  a: (p: AnchorHTMLAttributes<HTMLAnchorElement>) => <a className="text-primary hover:text-primary/80 underline underline-offset-2 transition-colors" target="_blank" rel="noopener noreferrer" {...p} />,
  hr: () => <hr className="my-4 border-border/50" />,
  blockquote: (p: HTMLAttributes<HTMLQuoteElement>) => <blockquote className="my-2 border-l-2 border-primary/30 pl-4 text-muted-foreground italic" {...p} />,
};

export interface MarkdownProps {
  children: string;
  className?: string;
  components?: Record<string, ComponentType>;
}

export function Markdown({ children, className, components }: MarkdownProps) {
  const merged = components ? { ...defaultComponents, ...components } : defaultComponents;
  return (
    <div className={cn("break-words text-[14px] leading-[1.7]", className)}>
      <ReactMarkdown components={merged}>{children}</ReactMarkdown>
    </div>
  );
}
