import { createContext, useContext, useState, useCallback, type ReactNode, type CSSProperties } from "react";
import { cn } from "../lib/utils";

interface SidebarContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
  toggle: () => void;
}

const SidebarContext = createContext<SidebarContextValue>({ open: true, setOpen: () => {}, toggle: () => {} });

export function useSidebar() {
  return useContext(SidebarContext);
}

export function AppShell({ children, defaultOpen = true, sidebarWidth = 256, className }: {
  children: ReactNode; defaultOpen?: boolean; sidebarWidth?: number; className?: string;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const toggle = useCallback(() => setOpen((o) => !o), []);

  return (
    <SidebarContext.Provider value={{ open, setOpen, toggle }}>
      <div
        className={cn("flex h-screen w-screen overflow-hidden bg-background", className)}
        style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties}
      >
        {children}
      </div>
    </SidebarContext.Provider>
  );
}

export function AppShellSidebar({ children, className }: { children: ReactNode; className?: string }) {
  const { open } = useSidebar();
  return (
    <aside className={cn(
      "flex h-full flex-col border-r bg-sidebar text-sidebar-foreground transition-[width] duration-200 ease-in-out overflow-hidden",
      open ? "w-[var(--sidebar-width)]" : "w-0",
      className,
    )}>
      <div className="flex h-full w-[var(--sidebar-width)] flex-col">{children}</div>
    </aside>
  );
}

export function AppShellMain({ children, className }: { children: ReactNode; className?: string }) {
  return <main className={cn("flex-1 overflow-auto", className)}>{children}</main>;
}
