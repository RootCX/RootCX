import { cn } from "@/lib/utils";

interface Property {
    name: string;
    type: string;
    required?: boolean;
    default?: string;
    description: string;
}

interface PropertiesTableProps {
    properties: Property[];
    className?: string;
}

export function PropertiesTable({ properties, className }: PropertiesTableProps) {
    return (
        <div className={cn("my-6 overflow-x-auto rounded-lg border border-border", className)}>
            <table className="w-full text-sm">
                <thead>
                    <tr className="border-b border-border bg-[#0d0d0d]">
                        <th className="px-4 py-3 text-left font-semibold text-foreground">Field</th>
                        <th className="px-4 py-3 text-left font-semibold text-foreground">Type</th>
                        <th className="px-4 py-3 text-left font-semibold text-foreground">Required</th>
                        <th className="px-4 py-3 text-left font-semibold text-foreground">Description</th>
                    </tr>
                </thead>
                <tbody>
                    {properties.map((prop, i) => (
                        <tr
                            key={i}
                            className="border-b border-border/50 last:border-0 hover:bg-white/[0.02] transition-colors"
                        >
                            <td className="px-4 py-3 font-mono text-xs text-primary">{prop.name}</td>
                            <td className="px-4 py-3">
                                <span className="rounded bg-[#1e1e2e] px-1.5 py-0.5 font-mono text-xs text-muted-foreground">
                                    {prop.type}
                                </span>
                            </td>
                            <td className="px-4 py-3">
                                {prop.required ? (
                                    <span className="text-xs text-green-400">Yes</span>
                                ) : (
                                    <span className="text-xs text-muted-foreground/50">No</span>
                                )}
                            </td>
                            <td className="px-4 py-3 text-muted-foreground leading-relaxed">
                                {prop.description}
                                {prop.default && (
                                    <span className="ml-1 text-xs text-muted-foreground/60">
                                        Default: <code className="font-mono">{prop.default}</code>
                                    </span>
                                )}
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </div>
    );
}
