import { cn } from "@/lib/utils";
import { Info, AlertTriangle, CheckCircle, Lightbulb } from "lucide-react";

type CalloutVariant = "info" | "warning" | "tip" | "note";

interface CalloutProps {
    variant?: CalloutVariant;
    title?: string;
    children: React.ReactNode;
    className?: string;
}

const variants = {
    info: {
        icon: Info,
        border: "border-blue-500/30",
        bg: "bg-blue-500/5",
        iconColor: "text-blue-400",
        titleColor: "text-blue-300",
    },
    warning: {
        icon: AlertTriangle,
        border: "border-yellow-500/30",
        bg: "bg-yellow-500/5",
        iconColor: "text-yellow-400",
        titleColor: "text-yellow-300",
    },
    tip: {
        icon: Lightbulb,
        border: "border-green-500/30",
        bg: "bg-green-500/5",
        iconColor: "text-green-400",
        titleColor: "text-green-300",
    },
    note: {
        icon: Info,
        border: "border-purple-500/30",
        bg: "bg-purple-500/5",
        iconColor: "text-purple-400",
        titleColor: "text-purple-300",
    },
};

export function Callout({ variant = "info", title, children, className }: CalloutProps) {
    const v = variants[variant];
    const Icon = v.icon;

    return (
        <div className={cn("my-6 flex gap-3 rounded-lg border p-4", v.border, v.bg, className)}>
            <Icon className={cn("mt-0.5 h-4 w-4 shrink-0", v.iconColor)} />
            <div className="flex flex-col gap-1 text-sm">
                {title && <p className={cn("font-semibold", v.titleColor)}>{title}</p>}
                <div className="text-muted-foreground leading-relaxed [&_code]:rounded [&_code]:bg-white/5 [&_code]:px-1 [&_code]:py-0.5 [&_code]:font-mono [&_code]:text-xs">
                    {children}
                </div>
            </div>
        </div>
    );
}
