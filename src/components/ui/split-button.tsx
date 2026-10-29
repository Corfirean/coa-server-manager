import { Button, type ButtonProps } from "./button";

/** Two explicit actions, each with its own enabled state and keyboard focus. */
export function SplitButton({ label, primary, secondary }: {
  label: string;
  primary: ButtonProps;
  secondary: ButtonProps;
}) {
  return <div role="group" aria-label={label} className="inline-flex max-w-full items-stretch rounded-md">
    <Button type="button" variant="primary" {...primary}
      className="h-auto min-h-12 min-w-0 rounded-r-none px-5 py-2 text-sm whitespace-normal" />
    <Button type="button" variant="secondary" {...secondary}
      className="h-auto min-h-12 min-w-0 rounded-l-none border-l border-l-gold/30 px-4 py-2 text-sm whitespace-normal" />
  </div>;
}
