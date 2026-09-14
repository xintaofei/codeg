import * as React from "react"

import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      // Settings forms get heuristic-detected as address fields — a 名称
      // label alone is enough for Chromium — and WebView2 then pops
      // "save this info?" bubbles mid-form. These inputs are app UI, not
      // profile fields, so the off default is right app-wide; a caller that
      // genuinely wants the browser's autofill passes its own value, and
      // the spread below lets it win.
      autoComplete="off"
      data-slot="input"
      className={cn(
        "bg-input/30 border-input focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 h-9 rounded-4xl border px-3 py-1 text-base transition-colors file:h-7 file:text-sm file:font-medium focus-visible:ring-[3px] aria-invalid:ring-[3px] md:text-sm file:text-foreground placeholder:text-muted-foreground w-full min-w-0 outline-none file:inline-flex file:border-0 file:bg-transparent disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 read-only:bg-muted/40 read-only:text-muted-foreground read-only:cursor-default",
        className
      )}
      {...props}
    />
  )
}

export { Input }
