import * as React from "react";
import { IconAlertTriangle } from "@tabler/icons-react";
import { cn } from "../lib/utils";
import { Button } from "../primitives/button";

interface ErrorStateProps {
  message?: string;
  onRetry?: () => void;
  className?: string;
}

export function ErrorState({ message = "Something went wrong", onRetry, className }: ErrorStateProps) {
  return (
    <div className={cn("flex flex-col items-center justify-center py-12 text-center", className)}>
      <IconAlertTriangle className="mb-3 h-8 w-8 text-destructive" />
      <p className="text-sm text-muted-foreground">{message}</p>
      {onRetry && (
        <Button variant="outline" size="sm" onClick={onRetry} className="mt-4">
          Try again
        </Button>
      )}
    </div>
  );
}
