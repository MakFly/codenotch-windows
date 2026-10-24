import { SlidersHorizontal } from "lucide-react";
import { Select, SelectContent, SelectTrigger, SelectValue } from "./ui/select";
import { type Provider } from "../lib/types";
import glyphs from "../lib/glyphs.json";
export { SelectItem } from "./ui/select";
export function Mark({ type }: { type: Provider }) {
  const key = type === "codex" ? "openai" : type;
  return (
    <span className="provider-mark">
      {key === "custom" ? (
        <SlidersHorizontal size={20} />
      ) : (
        <svg viewBox="0 0 1 1" aria-hidden="true">
          <path d={glyphs[key]} fill="currentColor" fillRule="evenodd" />
        </svg>
      )}
    </span>
  );
}
export function Toggle({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={`toggle ${checked ? "checked" : ""}`}
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span />
    </button>
  );
}
export function Choice({
  id,
  value,
  onChange,
  children,
  disabled = false,
}: {
  id: string;
  value: string;
  onChange: (v: string) => void;
  children: React.ReactNode;
  disabled?: boolean;
}) {
  return (
    <Select value={value} onValueChange={onChange} disabled={disabled}>
      <SelectTrigger id={id}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>{children}</SelectContent>
    </Select>
  );
}
