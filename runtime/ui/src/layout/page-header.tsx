import * as React from "react";
import { IconArrowLeft } from "@tabler/icons-react";
import { cn } from "../lib/utils";

interface PageHeaderProps {
  title: string;
  description?: string;
  breadcrumbs?: { label: string; onClick?: () => void }[];
  actions?: React.ReactNode;
  onBack?: () => void;
  className?: string;
}

export function PageHeader({ title, description, breadcrumbs, actions, onBack, className }: PageHeaderProps) {
  return (
    <div className={cn("flex flex-col gap-2 pb-4 sm:gap-1 sm:pb-6", className)}>
      {breadcrumbs && breadcrumbs.length > 0 && (
        <nav className="flex flex-wrap items-center gap-1 text-sm text-muted-foreground">
          {breadcrumbs.map((crumb, i) => (
            <React.Fragment key={i}>
              {i > 0 && <span className="mx-1">/</span>}
              {crumb.onClick ? (
                <button type="button" onClick={crumb.onClick} className="hover:text-foreground transition-colors">
                  {crumb.label}
                </button>
              ) : (
                <span>{crumb.label}</span>
              )}
            </React.Fragment>
          ))}
        </nav>
      )}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
        <div className="flex min-w-0 items-start gap-3">
          {onBack && (
            <button
              type="button"
              onClick={onBack}
              aria-label="Go back"
              className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-md hover:bg-accent transition-colors"
            >
              <IconArrowLeft className="h-4 w-4" />
            </button>
          )}
          <div className="min-w-0">
            <h1 className="truncate text-xl font-semibold tracking-tight sm:text-2xl">{title}</h1>
            {description && <p className="text-sm text-muted-foreground">{description}</p>}
          </div>
        </div>
        {actions && (
          <div className="flex flex-wrap items-center gap-2 sm:flex-nowrap sm:justify-end">{actions}</div>
        )}
      </div>
    </div>
  );
}
