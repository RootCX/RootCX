import {
  Group,
  Panel,
  Separator,
  type GroupProps,
  type PanelProps,
  type SeparatorProps,
} from "react-resizable-panels";

import { cn } from "@/lib/utils";

function ResizablePanelGroup({
  className,
  orientation,
  ...props
}: GroupProps) {
  return (
    <Group
      className={cn(
        "flex h-full w-full",
        orientation === "vertical" && "flex-col",
        className,
      )}
      orientation={orientation}
      {...props}
    />
  );
}

function ResizablePanel({ ...props }: PanelProps) {
  return <Panel {...props} />;
}

function ResizableHandle({
  className,
  ...props
}: SeparatorProps) {
  return (
    <Separator
      className={cn(
        "bg-border focus-visible:ring-ring relative flex w-px items-center justify-center after:absolute after:inset-y-0 after:left-1/2 after:w-1 after:-translate-x-1/2 focus-visible:ring-1 focus-visible:ring-offset-1 focus-visible:outline-hidden aria-[orientation=horizontal]:h-px aria-[orientation=horizontal]:w-full aria-[orientation=horizontal]:after:left-0 aria-[orientation=horizontal]:after:h-1 aria-[orientation=horizontal]:after:w-full aria-[orientation=horizontal]:after:translate-x-0 aria-[orientation=horizontal]:after:-translate-y-1/2",
        className,
      )}
      {...props}
    />
  );
}

export { ResizablePanelGroup, ResizablePanel, ResizableHandle };
