import type { ButtonHTMLAttributes } from "react";

export function Button(
  props: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "ghost" | "cta" }
) {
  const { variant = "primary", className, ...rest } = props;
  const cls = ["btn", variant, className].filter(Boolean).join(" ");
  return <button {...rest} className={cls} />;
}
