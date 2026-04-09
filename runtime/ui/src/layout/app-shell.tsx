import {
  createContext, useContext, useState, useCallback, useEffect, useLayoutEffect, useMemo,
  type ReactNode, type CSSProperties,
} from "react";
import { IconMenu2 } from "@tabler/icons-react";
import { cn } from "../lib/utils";
import { Button } from "../primitives/button";

const HEADER_CLS = "sticky top-0 z-30 flex h-12 flex-shrink-0 items-center gap-2 border-b bg-background/95 px-3 backdrop-blur supports-[backdrop-filter]:bg-background/70";

interface SidebarContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
  openMobile: boolean;
  setOpenMobile: (open: boolean) => void;
  isMobile: boolean;
  toggle: () => void;
  /** True when app rendered its own <AppShellHeader>. Suppresses auto header. */
  hasCustomHeader: boolean;
  setHasCustomHeader: (v: boolean) => void;
}

const SidebarContext = createContext<SidebarContextValue | null>(null);

export function useSidebar() {
  const ctx = useContext(SidebarContext);
  if (!ctx) throw new Error("useSidebar must be used within <AppShell>");
  return ctx;
}

/** Non-throwing variant — returns `null` if not inside an <AppShell>. */
export function useSidebarOptional() {
  return useContext(SidebarContext);
}

const MOBILE_QUERY = "(max-width: 767px)";

function useIsMobile() {
  const [isMobile, setIsMobile] = useState(() =>
    typeof window === "undefined" ? false : window.matchMedia(MOBILE_QUERY).matches,
  );
  useEffect(() => {
    const mql = window.matchMedia(MOBILE_QUERY);
    const onChange = () => setIsMobile(mql.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);
  return isMobile;
}

export function AppShell({
  children,
  defaultOpen = true,
  sidebarWidth = 256,
  className,
}: {
  children: ReactNode;
  defaultOpen?: boolean;
  sidebarWidth?: number;
  className?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [openMobile, setOpenMobile] = useState(false);
  const [hasCustomHeader, setHasCustomHeader] = useState(false);
  const isMobile = useIsMobile();

  useEffect(() => {
    if (!isMobile && openMobile) setOpenMobile(false);
  }, [isMobile, openMobile]);

  const toggle = useCallback(() => {
    if (isMobile) setOpenMobile((o) => !o);
    else setOpen((o) => !o);
  }, [isMobile]);

  const ctx = useMemo(
    () => ({ open, setOpen, openMobile, setOpenMobile, isMobile, toggle, hasCustomHeader, setHasCustomHeader }),
    [open, openMobile, isMobile, toggle, hasCustomHeader],
  );

  return (
    <SidebarContext.Provider value={ctx}>
      <div
        className={cn("flex h-[100dvh] w-full overflow-hidden bg-background", className)}
        style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties}
      >
        {children}
      </div>
    </SidebarContext.Provider>
  );
}

export function AppShellSidebar({ children, className }: { children: ReactNode; className?: string }) {
  const { open, openMobile, setOpenMobile, isMobile } = useSidebar();

  // Avoids double-mounting children (effects, queries, state).
  if (isMobile) {
    return (
      <>
        <div
          onClick={() => setOpenMobile(false)}
          aria-hidden
          className={cn(
            "fixed inset-0 z-40 bg-black/50 transition-opacity duration-200",
            openMobile ? "opacity-100" : "pointer-events-none opacity-0",
          )}
        />
        <aside
          role="dialog"
          aria-modal="true"
          aria-hidden={!openMobile}
          className={cn(
            "fixed inset-y-0 left-0 z-50 flex w-[var(--sidebar-width)] max-w-[85vw] flex-col border-r bg-sidebar text-sidebar-foreground shadow-xl transition-transform duration-200 ease-in-out",
            openMobile ? "translate-x-0" : "-translate-x-full",
            className,
          )}
        >
          {children}
        </aside>
      </>
    );
  }

  return (
    <aside
      className={cn(
        "flex h-full flex-col border-r bg-sidebar text-sidebar-foreground transition-[width] duration-200 ease-in-out overflow-hidden",
        open ? "w-[var(--sidebar-width)]" : "w-0",
        className,
      )}
    >
      <div className="flex h-full w-[var(--sidebar-width)] flex-col">{children}</div>
    </aside>
  );
}

export function AppShellMain({ children, className }: { children: ReactNode; className?: string }) {
  const { isMobile, hasCustomHeader } = useSidebar();
  const showAutoHeader = isMobile && !hasCustomHeader;
  return (
    <main className={cn("flex min-w-0 flex-1 flex-col overflow-auto", className)}>
      {showAutoHeader && (
        <header className={HEADER_CLS}><SidebarTrigger /></header>
      )}
      {children}
    </main>
  );
}

export function SidebarTrigger({ className, label = "Toggle navigation" }: { className?: string; label?: string }) {
  const { toggle } = useSidebar();
  return (
    <Button variant="ghost" size="icon" onClick={toggle} aria-label={label} className={className}>
      <IconMenu2 className="h-5 w-5" strokeWidth={1.75} />
    </Button>
  );
}

/** Sticky top bar. Hidden on md+ by default; pass `alwaysVisible` to show on all breakpoints. */
export function AppShellHeader({
  children,
  className,
  alwaysVisible = false,
}: {
  children: ReactNode;
  className?: string;
  alwaysVisible?: boolean;
}) {
  const { setHasCustomHeader } = useSidebar();
  // Synchronous to avoid a one-frame flash of the auto-header.
  useLayoutEffect(() => {
    setHasCustomHeader(true);
    return () => setHasCustomHeader(false);
  }, [setHasCustomHeader]);

  return (
    <header className={cn(HEADER_CLS, !alwaysVisible && "md:hidden", className)}>
      {children}
    </header>
  );
}
