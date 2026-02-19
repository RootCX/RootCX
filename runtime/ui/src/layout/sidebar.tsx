import * as React from "react";
import { cn } from "../lib/utils";
import { IconChevronDown } from "@tabler/icons-react";

interface SidebarProps {
  children: React.ReactNode;
  header?: React.ReactNode;
  footer?: React.ReactNode;
  className?: string;
}

export function Sidebar({ children, header, footer, className }: SidebarProps) {
  return (
    <div className={cn("flex h-full flex-col", className)}>
      {header && <div className="flex-shrink-0 border-b border-sidebar-border p-4">{header}</div>}
      <nav className="flex-1 overflow-auto p-2">{children}</nav>
      {footer && <div className="flex-shrink-0 border-t border-sidebar-border p-3">{footer}</div>}
    </div>
  );
}

interface SidebarSectionProps {
  title?: string;
  collapsible?: boolean;
  defaultOpen?: boolean;
  children: React.ReactNode;
  className?: string;
}

export function SidebarSection({ title, collapsible = false, defaultOpen = true, children, className }: SidebarSectionProps) {
  const [open, setOpen] = React.useState(defaultOpen);

  return (
    <div className={cn("py-1", className)}>
      {title && (
        <button
          type="button"
          className={cn(
            "flex w-full items-center px-2 py-1.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground",
            collapsible && "cursor-pointer hover:text-foreground",
          )}
          onClick={collapsible ? () => setOpen((o) => !o) : undefined}
          disabled={!collapsible}
        >
          {collapsible && (
            <IconChevronDown
              className={cn("mr-1 h-3 w-3 transition-transform", !open && "-rotate-90")}
            />
          )}
          {title}
        </button>
      )}
      {open && <div className="space-y-0.5">{children}</div>}
    </div>
  );
}

interface SidebarItemProps {
  icon?: React.ReactNode;
  label: string;
  badge?: React.ReactNode;
  active?: boolean;
  onClick?: () => void;
  className?: string;
}

export function SidebarItem({ icon, label, badge, active = false, onClick, className }: SidebarItemProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-3 rounded-md px-2 py-1.5 text-sm font-medium transition-colors",
        active
          ? "bg-sidebar-accent text-sidebar-accent-foreground"
          : "text-sidebar-foreground/80 hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
        className,
      )}
    >
      {icon && <span className="flex-shrink-0 [&_svg]:h-4 [&_svg]:w-4">{icon}</span>}
      <span className="flex-1 truncate text-left">{label}</span>
      {badge && <span className="flex-shrink-0">{badge}</span>}
    </button>
  );
}
