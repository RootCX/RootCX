import { forwardRef, type ComponentRef, type ComponentPropsWithoutRef } from "react";
import { Root, List, Trigger, Content } from "@radix-ui/react-tabs";
import { cn } from "../lib/utils";

const Tabs = Root;

const TabsList = forwardRef<ComponentRef<typeof List>, ComponentPropsWithoutRef<typeof List>>(
  ({ className, ...props }, ref) => (
    <List
      ref={ref}
      className={cn("inline-flex h-9 items-center gap-1 border-b border-border bg-transparent px-1", className)}
      {...props}
    />
  ),
);

const TabsTrigger = forwardRef<ComponentRef<typeof Trigger>, ComponentPropsWithoutRef<typeof Trigger>>(
  ({ className, ...props }, ref) => (
    <Trigger
      ref={ref}
      className={cn(
        "group relative inline-flex h-full items-center justify-center whitespace-nowrap px-3 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none disabled:pointer-events-none disabled:opacity-50 data-[state=active]:text-foreground after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 after:scale-x-0 after:bg-primary after:transition-transform data-[state=active]:after:scale-x-100",
        className,
      )}
      {...props}
    />
  ),
);

const TabsContent = forwardRef<ComponentRef<typeof Content>, ComponentPropsWithoutRef<typeof Content>>(
  ({ className, ...props }, ref) => (
    <Content ref={ref} className={cn("mt-2 focus-visible:outline-none", className)} {...props} />
  ),
);

export { Tabs, TabsList, TabsTrigger, TabsContent };
