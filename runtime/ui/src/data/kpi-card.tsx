import * as React from "react";
import { IconTrendingUp, IconTrendingDown } from "@tabler/icons-react";
import { cn } from "../lib/utils";
import { Card, CardContent } from "../primitives/card";

interface KPICardProps {
  label: string;
  value: string | number;
  trend?: { value: number; label?: string };
  icon?: React.ReactNode;
  className?: string;
}

export function KPICard({ label, value, trend, icon, className }: KPICardProps) {
  return (
    <Card className={cn("p-0", className)}>
      <CardContent className="p-6">
        <div className="flex items-center justify-between">
          <p className="text-sm font-medium text-muted-foreground">{label}</p>
          {icon && <span className="text-muted-foreground [&_svg]:h-4 [&_svg]:w-4">{icon}</span>}
        </div>
        <div className="mt-2 flex items-baseline gap-2">
          <span className="text-2xl font-bold">{value}</span>
          {trend && (
            <span
              className={cn(
                "inline-flex items-center gap-0.5 text-xs font-medium",
                trend.value > 0 ? "text-green-500" : trend.value < 0 ? "text-destructive" : "text-muted-foreground",
              )}
            >
              {trend.value > 0 ? <IconTrendingUp className="h-3 w-3" /> : trend.value < 0 ? <IconTrendingDown className="h-3 w-3" /> : null}
              {trend.value > 0 ? "+" : ""}{trend.value}%
              {trend.label && <span className="ml-1 text-muted-foreground">{trend.label}</span>}
            </span>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
