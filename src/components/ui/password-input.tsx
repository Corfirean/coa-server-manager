import * as React from "react";
import { Eye, EyeOff } from "lucide-react";
import { cn } from "@/lib/utils";
import { useT } from "@/i18n";

/**
 * A password field with its own show/hide button. The browser's built-in one only exists while the field is focused and
 * comes back only after the field is emptied, so people could not look at a password they had already typed.
 */
export const PasswordInput = React.forwardRef<HTMLInputElement, Omit<React.InputHTMLAttributes<HTMLInputElement>, "type">>(
  ({ className, ...props }, ref) => {
    const t = useT();
    const [shown, setShown] = React.useState(false);
    return (
      <div className="relative inline-block">
        <input ref={ref} type={shown ? "text" : "password"} className={cn(className, "pr-11 [&::-ms-reveal]:hidden")} {...props} />
        <button
          type="button"
          onClick={() => setShown((v) => !v)}
          aria-label={shown ? t("pw.hide") : t("pw.show")}
          aria-pressed={shown}
          title={shown ? t("pw.hide") : t("pw.show")}
          className="absolute inset-y-0 right-0 flex w-10 cursor-pointer items-center justify-center text-muted hover:text-ink"
        >
          {shown ? <EyeOff className="h-4 w-4" aria-hidden /> : <Eye className="h-4 w-4" aria-hidden />}
        </button>
      </div>
    );
  },
);
PasswordInput.displayName = "PasswordInput";
