// Shared shadcn-style Accordion, powered by Radix for keyboard support and ARIA state.
import * as React from "react";
import * as Primitive from "@radix-ui/react-accordion";
import { ChevronDown } from "lucide-react";
import { clsx } from "clsx";

/**
 * The open section of a single-open accordion ("" when all are closed), remembered across
 * visits on this PC so reopening the settings lands where the user left off.
 */
export function useOpenSection(key: string, initial: string) {
  const storageKey = `codenotch-section-${key}`;
  const [open, setOpen] = React.useState<string>(() => {
    try {
      return localStorage.getItem(storageKey) ?? initial;
    } catch {
      return initial;
    }
  });
  const change = (next: string) => {
    setOpen(next);
    try { localStorage.setItem(storageKey, next); } catch { /* Storage may be disabled. */ }
  };
  return [open, change] as const;
}

export const Accordion = React.forwardRef<
  React.ElementRef<typeof Primitive.Root>,
  React.ComponentPropsWithoutRef<typeof Primitive.Root>
>(({ className, ...props }, ref) => (
  <Primitive.Root ref={ref} className={clsx("accordion", className)} {...props} />
));
Accordion.displayName = "Accordion";

export const AccordionItem = React.forwardRef<
  React.ElementRef<typeof Primitive.Item>,
  React.ComponentPropsWithoutRef<typeof Primitive.Item>
>(({ className, ...props }, ref) => (
  <Primitive.Item ref={ref} className={clsx("accordion-item", className)} {...props} />
));
AccordionItem.displayName = "AccordionItem";

export const AccordionTrigger = React.forwardRef<
  React.ElementRef<typeof Primitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof Primitive.Trigger>
>(({ className, children, ...props }, ref) => (
  <Primitive.Header className="accordion-header">
    <Primitive.Trigger ref={ref} className={clsx("accordion-trigger", className)} {...props}>
      {children}
      <ChevronDown size={16} className="accordion-chevron" aria-hidden="true" />
    </Primitive.Trigger>
  </Primitive.Header>
));
AccordionTrigger.displayName = "AccordionTrigger";

export const AccordionContent = React.forwardRef<
  React.ElementRef<typeof Primitive.Content>,
  React.ComponentPropsWithoutRef<typeof Primitive.Content>
>(({ className, children, ...props }, ref) => (
  <Primitive.Content ref={ref} className="accordion-content" {...props}>
    <div className={clsx("accordion-body", className)}>{children}</div>
  </Primitive.Content>
));
AccordionContent.displayName = "AccordionContent";
