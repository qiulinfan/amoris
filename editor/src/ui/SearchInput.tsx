import { Search, X } from "lucide-react";
import { forwardRef } from "react";

interface Props {
  value: string;
  onChange(v: string): void;
  placeholder?: string;
  onKeyDown?(e: React.KeyboardEvent<HTMLInputElement>): void;
}

export const SearchInput = forwardRef<HTMLInputElement, Props>(function SearchInput({ value, onChange, placeholder, onKeyDown }, ref) {
  return (
    <div className="search-input">
      <Search size={13} strokeWidth={1.8} />
      <input
        ref={ref}
        value={value}
        placeholder={placeholder ?? "Search"}
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && value) {
            e.stopPropagation();
            onChange("");
          }
          onKeyDown?.(e);
        }}
      />
      {value && (
        <button type="button" className="search-clear" aria-label="Clear" onClick={() => onChange("")}>
          <X size={12} />
        </button>
      )}
    </div>
  );
});
