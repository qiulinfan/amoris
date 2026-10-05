import type { ButtonHTMLAttributes, ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { formatKeys } from "../commands/keys";
import { cx } from "./cx";

type Variant = "default" | "primary" | "ghost" | "danger" | "subtle";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  icon?: LucideIcon;
  size?: "sm" | "md";
  children?: ReactNode;
}

export function Button({ variant = "default", icon: Icon, size = "md", className, children, ...rest }: ButtonProps) {
  return (
    <button type="button" className={cx("btn", `btn-${variant}`, `btn-${size}`, className)} {...rest}>
      {Icon && <Icon size={size === "sm" ? 13 : 14} strokeWidth={1.8} />}
      {children !== undefined && <span>{children}</span>}
    </button>
  );
}

interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> {
  icon: LucideIcon;
  label: string;
  keys?: string;
  active?: boolean;
  size?: "sm" | "md" | "lg";
  tone?: "default" | "play" | "danger" | "accent";
}

export function IconButton({ icon: Icon, label, keys, active, size = "md", tone = "default", className, ...rest }: IconButtonProps) {
  const px = size === "sm" ? 13 : size === "lg" ? 17 : 15;
  return (
    <button
      type="button"
      className={cx("icon-btn", `icon-btn-${size}`, `tone-${tone}`, active && "is-active", className)}
      aria-label={label}
      aria-pressed={active}
      data-tip={keys ? `${label}  ${formatKeys(keys)}` : label}
      {...rest}
    >
      <Icon size={px} strokeWidth={1.8} />
    </button>
  );
}
