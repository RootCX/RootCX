import type { EntitySchema, FieldSchema } from "../runner.js";

function formatField(f: FieldSchema): string {
    let s = `${f.name}(${f.type}`;
    if (f.required) s += ", required";
    if (f.enumValues?.length) s += `: ${f.enumValues.join("|")}`;
    if (f.references) s += ` → ${f.references.entity}`;
    s += ")";
    return s;
}

export function formatSchema(entities: EntitySchema[]): string {
    if (!entities.length) return "";
    const lines = entities.map(
        (e) => `- ${e.entityName}: ${e.fields.map(formatField).join(", ")}`,
    );
    return `\nSchema:\n${lines.join("\n")}`;
}
