import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "./button";

export function Dialog({ title, description, open, onOpenChange, onEscapeKeyDown, onCloseAutoFocus, className, children }: {
  title: string;
  description: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onEscapeKeyDown?: (event: KeyboardEvent) => void;
  onCloseAutoFocus?: (event: Event) => void;
  className?: string;
  children: ReactNode;
}) {
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="dialog-overlay fixed inset-0" />
        <DialogPrimitive.Content className={`dialog-content fixed ${className ?? ""}`} onEscapeKeyDown={onEscapeKeyDown} onCloseAutoFocus={onCloseAutoFocus}>
          <DialogPrimitive.Title className="dialog-title">{title}</DialogPrimitive.Title>
          <DialogPrimitive.Description className="muted dialog-description">{description}</DialogPrimitive.Description>
          {children}
          <DialogPrimitive.Close asChild>
            <Button className="dialog-close" variant="ghost" size="icon" aria-label="Close dialog"><X size={18} /></Button>
          </DialogPrimitive.Close>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
