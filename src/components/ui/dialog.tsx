import * as React from "react";
import * as Primitive from "@radix-ui/react-dialog";
import { clsx } from "clsx";

export const Dialog = Primitive.Root;
export const DialogTitle = Primitive.Title;
export const DialogDescription = Primitive.Description;
export const DialogClose = Primitive.Close;
export const DialogContent = React.forwardRef<
  React.ElementRef<typeof Primitive.Content>,
  React.ComponentPropsWithoutRef<typeof Primitive.Content>
>(({ className, children, ...props }, ref) => (
  <Primitive.Portal>
    <Primitive.Overlay className="dialog-overlay">
      <Primitive.Content ref={ref} className={clsx("dialog-content", className)} {...props}>
        {children}
      </Primitive.Content>
    </Primitive.Overlay>
  </Primitive.Portal>
));
DialogContent.displayName = "DialogContent";
