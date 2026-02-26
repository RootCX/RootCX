import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const labelVariants = cva("font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70", {
  variants: {
    size: {
      default: "text-sm",
      sm: "text-xs",
      xs: "text-[10px] uppercase tracking-wider text-muted-foreground",
    },
  },
  defaultVariants: { size: "default" },
});

export const Label = React.forwardRef<
  HTMLLabelElement,
  React.LabelHTMLAttributes<HTMLLabelElement> & VariantProps<typeof labelVariants>
>(({ className, size, ...props }, ref) => (
  <label ref={ref} className={cn(labelVariants({ size, className }))} {...props} />
));
Label.displayName = "Label";
