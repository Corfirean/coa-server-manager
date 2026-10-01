import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md font-medium transition-colors disabled:pointer-events-none disabled:opacity-45 cursor-pointer",
  {
    variants: {
      variant: {
        primary: "bg-gold text-[#17130a] hover:bg-gold-strong",
        secondary: "bg-card-2 text-ink border border-line hover:bg-[#1f2226]",
        ghost: "text-muted hover:text-ink hover:bg-white/5",
        danger: "bg-bad/90 text-white hover:bg-bad",
      },
      size: {
        sm: "h-9 px-3 text-sm",
        md: "h-11 px-5 text-[15px]",
        xl: "h-16 px-10 text-lg tracking-wide",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, ...props }, ref) => (
    <button ref={ref} className={cn(buttonVariants({ variant, size }), className)} {...props} />
  ),
);
Button.displayName = "Button";
