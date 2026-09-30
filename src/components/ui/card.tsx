import * as React from "react";
import { cn } from "@/lib/utils";

export function Card({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn("rounded-card border border-line bg-card/90 shadow-[0_8px_30px_rgb(0_0_0/0.28)]", className)}
      {...props}
    />
  );
}
