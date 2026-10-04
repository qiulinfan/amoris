import { useState } from "react";

export function TextField({
  value,
  onCommit,
  placeholder,
  disabled,
  mono,
  maxLength,
}: {
  value: string;
  onCommit(v: string): void;
  placeholder?: string;
  disabled?: boolean;
  mono?: boolean;
  maxLength?: number;
}) {
  const [text, setText] = useState<string | null>(null);
  const commit = () => {
    if (text !== null && text !== value) onCommit(text);
    setText(null);
  };
  return (
    <input
      className={`text-field ${mono ? "mono" : ""}`}
      value={text ?? value}
      placeholder={placeholder}
      disabled={disabled}
      maxLength={maxLength}
      spellCheck={false}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape") {
          setText(null);
          requestAnimationFrame(() => e.currentTarget?.blur());
        }
      }}
    />
  );
}
