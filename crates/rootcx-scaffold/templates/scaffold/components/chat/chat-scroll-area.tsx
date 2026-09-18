import {
  type ReactNode,
  useState, useRef, useEffect, useCallback, useImperativeHandle, forwardRef,
} from "react";
import { cn } from "@/lib/utils";
import { Button } from "@rootcx/ui";
import { IconChevronDown } from "@tabler/icons-react";

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
          <Button
            type="button"
            size="icon"
            variant="outline"
            aria-label="Scroll to latest message"
            onClick={() => scrollToBottom()}
            className="absolute bottom-4 left-1/2 -translate-x-1/2"
          >
            <IconChevronDown />
          </Button>
        )}
      </div>
    );
  },
);
ChatScrollArea.displayName = "ChatScrollArea";

export { ChatScrollArea };
