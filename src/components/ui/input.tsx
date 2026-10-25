// Shared shadcn-style text input: one height, border and focus ring for every form.
import * as React from "react";
import { clsx } from "clsx";

export const Input = React.forwardRef<HTMLInputElement, React.ComponentProps<"input">>(
  ({ className, type = "text", ...props }, ref) => (
    <input ref={ref} type={type} className={clsx("ui-input", className)} {...props} />
  ),
);
Input.displayName = "Input";
