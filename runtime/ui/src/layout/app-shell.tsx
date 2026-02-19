import * as React from "react";
import { cn } from "../lib/utils";

interface SidebarContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
  toggle: () => void;
}

const SidebarContext = React.createContext<SidebarContextValue>({
  open: true,
  setOpen: () => {},
  toggle: () => {},
});

export function useSidebar() {
  return React.useContext(SidebarContext);
}

interface AppShellProps {
  children: React.ReactNode;
  defaultOpen?: boolean;
  sidebarWidth?: number;
  className?: string;
}

export function AppShell({ children, defaultOpen = true, sidebarWidth = 256, className }: AppShellProps) {
  const [open, setOpen] = React.useState(defaultOpen);
  const toggle = React.useCallback(() => setOpen((o) => !o), []);

  return (
    <SidebarContext.Provider value={{ open, setOpen, toggle }}>
      <div
        className={cn("flex h-screen w-screen overflow-hidden bg-background", className)}
        style={{ "--sidebar-width": `${sidebarWidth}px` } as React.CSSProperties}
      >
        {children}
      </div>
    </SidebarContext.Provider>
  );
}

interface AppShellSidebarProps {
  children: React.ReactNode;
  className?: string;
}

export function AppShellSidebar({ children, className }: AppShellSidebarProps) {
  const { open } = useSidebar();

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

interface AppShellMainProps {
  children: React.ReactNode;
  className?: string;
}

export function AppShellMain({ children, className }: AppShellMainProps) {
  return (
    <main className={cn("flex-1 overflow-auto", className)}>
      {children}
    </main>
  );
}
