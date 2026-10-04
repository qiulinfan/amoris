import { ChevronDown } from "lucide-react";

export function SelectField({
  value,
  options,
  onChange,
  disabled,
}: {
  value: string;
  options: { value: string; label?: string; doc?: string }[];
  onChange(v: string): void;
  disabled?: boolean;
}) {
  const doc = options.find((o) => o.value === value)?.doc;
  return (
    <div className="select-field" data-tip={doc}>
      <select value={value} disabled={disabled} onChange={(e) => onChange(e.target.value)}>
        {!options.some((o) => o.value === value) && <option value={value}>{value || "—"}</option>}
        {options.map((o) => (
          <option key={o.value} value={o.value} title={o.doc}>
            {o.label ?? o.value}
          </option>
        ))}
      </select>
      <ChevronDown size={12} className="select-chev" />
    </div>
  );
}
