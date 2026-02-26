import { cn } from "@/lib/utils";

export function ToggleDot({ active, disabled, onClick }: {
  active: boolean;
  disabled?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={disabled ? undefined : onClick}
      className={cn(
        "flex h-6 w-6 items-center justify-center rounded transition-colors",
        disabled ? "cursor-default" : "cursor-pointer hover:bg-accent",
      )}
    >
      <span className={cn(
        "h-3 w-3 rounded-full border-2 transition-colors",
        active
          ? "border-primary bg-primary"
          : disabled ? "border-muted-foreground/30" : "border-muted-foreground/50 hover:border-muted-foreground",
      )} />
    </button>
  );
}
