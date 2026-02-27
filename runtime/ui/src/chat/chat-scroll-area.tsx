import {
  type ReactNode, type RefObject,
  useState, useRef, useEffect, useCallback, useImperativeHandle, forwardRef,
} from "react";
import { cn } from "../lib/utils";

const BOTTOM_THRESHOLD = 30;

export function useAutoScroll() {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);
  const stickRef = useRef(true);
  const lastScrollTop = useRef(0);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const { scrollTop, scrollHeight, clientHeight } = el;
      if (scrollTop < lastScrollTop.current - 5) stickRef.current = false;
      if (scrollHeight - scrollTop - clientHeight < BOTTOM_THRESHOLD) stickRef.current = true;
      lastScrollTop.current = scrollTop;
      setIsAtBottom(stickRef.current);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    const content = contentRef.current;
    const scroll = scrollRef.current;
    if (!content || !scroll) return;
    const observer = new ResizeObserver(() => {
      if (stickRef.current) {
        scroll.scrollTop = scroll.scrollHeight;
        lastScrollTop.current = scroll.scrollTop;
      }
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  const scrollToBottom = useCallback((behavior: ScrollBehavior = "smooth") => {
    const el = scrollRef.current;
    if (!el) return;
    stickRef.current = true;
    setIsAtBottom(true);
    el.scrollTo({ top: el.scrollHeight, behavior });
  }, []);

  return { scrollRef, contentRef, isAtBottom, scrollToBottom };
}

export interface ChatScrollAreaHandle {
  scrollToBottom: (behavior?: ScrollBehavior) => void;
  isAtBottom: boolean;
}

export interface ChatScrollAreaProps {
  children: ReactNode;
  className?: string;
  contentClassName?: string;
  showScrollIndicator?: boolean;
}

const ChatScrollArea = forwardRef<ChatScrollAreaHandle, ChatScrollAreaProps>(
  ({ children, className, contentClassName, showScrollIndicator = true }, ref) => {
    const { scrollRef, contentRef, isAtBottom, scrollToBottom } = useAutoScroll();

    useImperativeHandle(ref, () => ({ scrollToBottom, isAtBottom }), [scrollToBottom, isAtBottom]);

    return (
      <div className={cn("relative min-h-0 overflow-hidden", className)}>
        <div ref={scrollRef} className="h-full overflow-y-auto overflow-x-hidden">
          <div ref={contentRef} className={contentClassName}>
            {children}
          </div>
        </div>
        {showScrollIndicator && !isAtBottom && (
          <button
            type="button"
            onClick={() => scrollToBottom()}
            className="absolute bottom-4 left-1/2 z-10 flex h-8 w-8 -translate-x-1/2 items-center justify-center rounded-full border border-border/60 bg-card shadow-lg shadow-black/20 text-muted-foreground transition-all hover:bg-muted hover:text-foreground"
          >
            <svg className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round"><path d="m6 9 6 6 6-6" /></svg>
          </button>
        )}
      </div>
    );
  },
);
ChatScrollArea.displayName = "ChatScrollArea";

export { ChatScrollArea };
