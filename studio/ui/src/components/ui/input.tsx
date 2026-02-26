import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const inputVariants = cva(
  "flex w-full rounded-md border border-input bg-transparent shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
  {
    variants: {
      size: {
        default: "h-9 px-3 py-1 text-sm",
        sm: "h-7 px-2.5 py-0.5 text-xs",
        xs: "h-6 px-2 py-0 text-[10px]",
      },
    },
    defaultVariants: { size: "default" },
  },
);

export const Input = React.forwardRef<
  HTMLInputElement,
  Omit<React.InputHTMLAttributes<HTMLInputElement>, "size"> & VariantProps<typeof inputVariants>
>(({ className, type, size, ...props }, ref) => (
  <input type={type} className={cn(inputVariants({ size, className }))} ref={ref} {...props} />
));
Input.displayName = "Input";
