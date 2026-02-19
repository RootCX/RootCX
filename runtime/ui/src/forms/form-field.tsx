import * as React from "react";
import { cn } from "../lib/utils";
import { Label } from "../primitives/label";
import { Input } from "../primitives/input";
import { Textarea } from "../primitives/textarea";
import { Switch } from "../primitives/switch";
import {
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "../primitives/select";

export type FieldType = "text" | "number" | "email" | "password" | "textarea" | "select" | "boolean" | "date";

export interface FieldDefinition {
  name: string;
  label: string;
  type: FieldType;
  required?: boolean;
  placeholder?: string;
  options?: { value: string; label: string }[];
}

interface FormFieldProps {
  field: FieldDefinition;
  value: unknown;
  onChange: (value: unknown) => void;
  error?: string;
  className?: string;
}

export function FormField({ field, value, onChange, error, className }: FormFieldProps) {
  const id = `field-${field.name}`;

  return (
    <div className={cn("space-y-2", className)}>
      <Label htmlFor={id}>
        {field.label}
        {field.required && <span className="ml-1 text-destructive">*</span>}
      </Label>

      {field.type === "textarea" ? (
        <Textarea
          id={id}
          placeholder={field.placeholder}
          value={(value as string) ?? ""}
          onChange={(e) => onChange(e.target.value)}
          className={error ? "border-destructive" : undefined}
        />
      ) : field.type === "select" && field.options ? (
        <Select value={(value as string) ?? ""} onValueChange={onChange}>
          <SelectTrigger id={id} className={error ? "border-destructive" : undefined}>
            <SelectValue placeholder={field.placeholder ?? "Select..."} />
          </SelectTrigger>
          <SelectContent>
            {field.options.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>{opt.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : field.type === "boolean" ? (
        <div className="flex items-center gap-2">
          <Switch id={id} checked={!!value} onCheckedChange={onChange} />
        </div>
      ) : (
        <Input
          id={id}
          type={field.type === "number" ? "number" : field.type === "date" ? "date" : field.type === "email" ? "email" : field.type === "password" ? "password" : "text"}
          placeholder={field.placeholder}
          value={(value as string | number) ?? ""}
          onChange={(e) => onChange(field.type === "number" ? (e.target.value === "" ? "" : Number(e.target.value)) : e.target.value)}
          className={error ? "border-destructive" : undefined}
        />
      )}

      {error && <p className="text-sm text-destructive">{error}</p>}
    </div>
  );
}
