import * as React from "react";
import { IconLoader2 } from "@tabler/icons-react";
import { cn } from "../lib/utils";

interface LoadingStateProps {
  variant?: "spinner" | "skeleton";
  rows?: number;
  className?: string;
}

export function LoadingState({ variant = "spinner", rows = 3, className }: LoadingStateProps) {
  if (variant === "skeleton") {
    return (
      <div className={cn("space-y-3", className)}>
        {Array.from({ length: rows }).map((_, i) => (
          <div key={i} className="h-4 animate-pulse rounded bg-muted" style={{ width: `${Math.max(40, 100 - i * 15)}%` }} />
        ))}
      </div>
    );
  }

  return (
    <div className={cn("flex items-center justify-center py-12", className)}>
      <IconLoader2 className="h-6 w-6 animate-spin text-muted-foreground" />
    </div>
  );
}
