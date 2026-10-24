// Shared shadcn-style Select composition, powered by Radix for focus and keyboard support.
import * as React from "react";
import * as Primitive from "@radix-ui/react-select";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { clsx } from "clsx";
export const Select = Primitive.Root;
export const SelectValue = Primitive.Value;
export const SelectGroup = Primitive.Group;
export const SelectTrigger = React.forwardRef<
  React.ElementRef<typeof Primitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof Primitive.Trigger>
>(({ className, children, ...props }, ref) => (
  <Primitive.Trigger
    ref={ref}
    className={clsx("select-trigger", className)}
    {...props}
  >
    {children}
    <Primitive.Icon asChild>
      <ChevronDown size={16} />
    </Primitive.Icon>
  </Primitive.Trigger>
));
SelectTrigger.displayName = "SelectTrigger";
export const SelectContent = React.forwardRef<
  React.ElementRef<typeof Primitive.Content>,
  React.ComponentPropsWithoutRef<typeof Primitive.Content>
>(({ className, children, ...props }, ref) => (
  <Primitive.Portal>
    <Primitive.Content
      ref={ref}
      position="popper"
      sideOffset={6}
      className={clsx("select-content", className)}
      {...props}
    >
      <Primitive.ScrollUpButton className="select-scroll">
        <ChevronUp size={16} />
      </Primitive.ScrollUpButton>
      <Primitive.Viewport>{children}</Primitive.Viewport>
      <Primitive.ScrollDownButton className="select-scroll">
        <ChevronDown size={16} />
      </Primitive.ScrollDownButton>
    </Primitive.Content>
  </Primitive.Portal>
));
SelectContent.displayName = "SelectContent";
export const SelectItem = React.forwardRef<
  React.ElementRef<typeof Primitive.Item>,
  React.ComponentPropsWithoutRef<typeof Primitive.Item>
>(({ className, children, ...props }, ref) => (
  <Primitive.Item
    ref={ref}
    className={clsx("select-item", className)}
    {...props}
  >
    <Primitive.ItemText>{children}</Primitive.ItemText>
    <Primitive.ItemIndicator>
      <Check size={15} />
    </Primitive.ItemIndicator>
  </Primitive.Item>
));
SelectItem.displayName = "SelectItem";
