import * as React from "react";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter,
} from "../primitives/dialog";
import { Button } from "../primitives/button";
import { FormField, type FieldDefinition } from "./form-field";
import { IconLoader2 } from "@tabler/icons-react";

interface FormDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  fields: FieldDefinition[];
  defaultValues?: Record<string, unknown>;
  onSubmit: (values: Record<string, unknown>) => Promise<void> | void;
  submitLabel?: string;
  destructive?: boolean;
}

export function FormDialog({
  open,
  onOpenChange,
  title,
  description,
  fields,
  defaultValues = {},
  onSubmit,
  submitLabel = "Save",
  destructive = false,
}: FormDialogProps) {
  const [values, setValues] = React.useState<Record<string, unknown>>(defaultValues);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [submitting, setSubmitting] = React.useState(false);

  React.useEffect(() => {
    if (open) {
      setValues(defaultValues);
      setErrors({});
    }
  }, [open]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();

    const newErrors: Record<string, string> = {};
    for (const field of fields) {
      if (field.required) {
        const v = values[field.name];
        if (v === undefined || v === null || v === "") {
          newErrors[field.name] = `${field.label} is required`;
        }
      }
    }

    if (Object.keys(newErrors).length > 0) {
      setErrors(newErrors);
      return;
    }

    setSubmitting(true);
    try {
      await onSubmit(values);
      onOpenChange(false);
    } catch {
      // caller handles errors
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            {description && <DialogDescription>{description}</DialogDescription>}
          </DialogHeader>
          <div className="grid gap-4 py-4">
            {fields.map((field) => (
              <FormField
                key={field.name}
                field={field}
                value={values[field.name]}
                onChange={(v) => {
                  setValues((prev) => ({ ...prev, [field.name]: v }));
                  setErrors((prev) => {
                    const next = { ...prev };
                    delete next[field.name];
                    return next;
                  });
                }}
                error={errors[field.name]}
              />
            ))}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={submitting}>
              Cancel
            </Button>
            <Button type="submit" variant={destructive ? "destructive" : "default"} disabled={submitting}>
              {submitting && <IconLoader2 className="h-4 w-4 animate-spin" />}
              {submitLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
