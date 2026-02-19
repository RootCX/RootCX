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
    <div className={cn("flex flex-col gap-1 pb-6", className)}>
      {breadcrumbs && breadcrumbs.length > 0 && (
        <nav className="flex items-center gap-1 text-sm text-muted-foreground">
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
      <div className="flex items-center justify-between gap-4">
        <div className="flex items-center gap-3">
          {onBack && (
            <button
              type="button"
              onClick={onBack}
              className="flex h-8 w-8 items-center justify-center rounded-md hover:bg-accent transition-colors"
            >
              <IconArrowLeft className="h-4 w-4" />
            </button>
          )}
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
            {description && <p className="text-sm text-muted-foreground">{description}</p>}
          </div>
        </div>
        {actions && <div className="flex items-center gap-2">{actions}</div>}
      </div>
    </div>
  );
}
