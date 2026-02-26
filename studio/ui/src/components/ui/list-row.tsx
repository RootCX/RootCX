import * as React from "react";
import { cn } from "@/lib/utils";

export const ListRow = React.forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, onClick, ...props }, ref) => (
    <div
      ref={ref}
      onClick={onClick}
      className={cn("flex items-center gap-2 rounded px-2 py-1", onClick && "cursor-pointer hover:bg-accent/30", className)}
      {...props}
    />
  ),
);
ListRow.displayName = "ListRow";
