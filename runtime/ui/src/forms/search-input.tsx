import * as React from "react";
import { IconSearch } from "@tabler/icons-react";
import { cn } from "../lib/utils";
import { Input } from "../primitives/input";

interface SearchInputProps {
  value?: string;
  onChange: (value: string) => void;
  placeholder?: string;
  debounceMs?: number;
  className?: string;
}

export function SearchInput({ value: controlledValue, onChange, placeholder = "Search...", debounceMs = 300, className }: SearchInputProps) {
  const [localValue, setLocalValue] = React.useState(controlledValue ?? "");
  const timeoutRef = React.useRef<ReturnType<typeof setTimeout>>(undefined);

  React.useEffect(() => {
    if (controlledValue !== undefined) {
      setLocalValue(controlledValue);
    }
  }, [controlledValue]);

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const v = e.target.value;
    setLocalValue(v);
    clearTimeout(timeoutRef.current);
    timeoutRef.current = setTimeout(() => onChange(v), debounceMs);
  };

  React.useEffect(() => {
    return () => clearTimeout(timeoutRef.current);
  }, []);

  return (
    <div className={cn("relative w-full sm:max-w-sm", className)}>
      <IconSearch className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
      <Input
        placeholder={placeholder}
        value={localValue}
        onChange={handleChange}
        className="pl-8"
      />
    </div>
  );
}
