import * as Menu from "@radix-ui/react-dropdown-menu";
import { Check, ChevronDown } from "lucide-react";
import { Children, isValidElement, type ComponentProps, type ReactNode } from "react";

type Option = ComponentProps<"option">;
type SelectProps = Pick<ComponentProps<"button">, "className" | "disabled" | "id" | "aria-label" | "aria-labelledby"> & {
  value: string | number;
  onValueChange: (value: string) => void;
  children: ReactNode;
};

export function Select({ value, onValueChange, children, className, ...button }: SelectProps) {
  const options = Children.toArray(children).flatMap((child) => {
    if (!isValidElement<Option>(child) || child.type !== "option") return [];
    const { value, disabled, children: label } = child.props;
    return [{ value: String(value ?? label), disabled, label }];
  });
  const selected = options.find((option) => option.value === String(value));

  return <Menu.Root modal={false}>
    <Menu.Trigger {...button} type="button" className={["version-picker-trigger", "select-trigger", className].filter(Boolean).join(" ")}>
      <span>{selected?.label ?? "Select an option"}</span><ChevronDown size={15} aria-hidden="true" />
    </Menu.Trigger>
    <Menu.Portal>
      <Menu.Content className="select-panel" align="start" sideOffset={5} collisionPadding={8}>
        <Menu.RadioGroup value={String(value)} onValueChange={onValueChange}>
          {options.map((option) => <Menu.RadioItem key={option.value} value={option.value} disabled={option.disabled}
            className="version-option select-option">
            <span>{option.label}</span>
            <Menu.ItemIndicator><Check size={13} aria-hidden="true" /></Menu.ItemIndicator>
          </Menu.RadioItem>)}
        </Menu.RadioGroup>
      </Menu.Content>
    </Menu.Portal>
  </Menu.Root>;
}
