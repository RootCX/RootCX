import { cn } from "../lib/utils";

const green = "bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-400";
const yellow = "bg-yellow-100 text-yellow-800 dark:bg-yellow-900/30 dark:text-yellow-400";
const gray = "bg-gray-100 text-gray-800 dark:bg-gray-900/30 dark:text-gray-400";
const red = "bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-400";
const blue = "bg-blue-100 text-blue-800 dark:bg-blue-900/30 dark:text-blue-400";

const statusColors: Record<string, string> = {
  active: green, open: green, completed: green, closed_won: green, success: green,
  pending: yellow, in_progress: yellow, qualified: yellow, proposal: yellow, warning: yellow,
  inactive: gray, closed: gray, draft: gray, lead: gray,
  error: red, failed: red, closed_lost: red, rejected: red,
  negotiation: blue, review: blue, info: blue,
};

interface StatusBadgeProps {
  status: string;
  className?: string;
}

export function StatusBadge({ status, className }: StatusBadgeProps) {
  const normalized = status.toLowerCase().replace(/[\s-]/g, "_");
  const color = statusColors[normalized] ?? gray;
  const display = status.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());

  return (
    <span className={cn("inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium", color, className)}>
      {display}
    </span>
  );
}
