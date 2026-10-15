// Shared primitives. Small on purpose — enough to keep the pages consistent
// without becoming a component library nobody asked for.

import { useState, type ReactNode } from "react";
import { Eye, EyeOff, Loader2 } from "lucide-react";

export function Button({
  children,
  variant = "primary",
  busy = false,
  className = "",
  ...rest
}: {
  children: ReactNode;
  variant?: "primary" | "ghost" | "danger";
  busy?: boolean;
} & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  const base =
    "inline-flex items-center justify-center gap-2 rounded-[var(--radius-control)] " +
    "px-3.5 py-2 text-sm font-medium transition-colors " +
    "disabled:opacity-50 disabled:cursor-not-allowed";

  const variants = {
    primary: "bg-accent text-white hover:bg-accent-hover",
    ghost:
      "border border-border bg-transparent text-ink-muted hover:text-ink hover:bg-surface",
    danger: "bg-danger-soft text-danger hover:brightness-110",
  };

  return (
    <button
      {...rest}
      disabled={rest.disabled || busy}
      className={`${base} ${variants[variant]} ${className}`}
    >
      {busy && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
      {children}
    </button>
  );
}

export function Field({
  label,
  hint,
  ...rest
}: { label: string; hint?: string } & React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-xs font-medium text-ink-muted">
        {label}
      </span>
      <input
        {...rest}
        className="w-full rounded-[var(--radius-control)] border border-border
                   bg-canvas px-3 py-2 text-sm text-ink outline-none
                   transition-colors placeholder:text-ink-faint
                   focus:border-accent"
      />
      {hint && <span className="mt-1 block text-xs text-ink-faint">{hint}</span>}
    </label>
  );
}

/// A password input with a reveal toggle.
///
/// Separate from `Field` rather than a flag on it, because the toggle needs
/// its own state and the input needs right padding to clear the button --
/// conditionally applying both inside `Field` makes that component worse for
/// every other use.
export function PasswordField({
  label,
  hint,
  ...rest
}: { label: string; hint?: string } & React.InputHTMLAttributes<HTMLInputElement>) {
  const [revealed, setRevealed] = useState(false);
  const Icon = revealed ? EyeOff : Eye;

  return (
    <label className="block">
      <span className="mb-1.5 block text-xs font-medium text-ink-muted">
        {label}
      </span>
      <div className="relative">
        <input
          {...rest}
          type={revealed ? "text" : "password"}
          className="w-full rounded-[var(--radius-control)] border border-border
                     bg-canvas py-2 pl-3 pr-10 text-sm text-ink outline-none
                     transition-colors placeholder:text-ink-faint
                     focus:border-accent"
        />
        <button
          type="button"
          onClick={() => setRevealed((shown) => !shown)}
          // Without this the button is a tab stop between the password field
          // and the submit button, which breaks typing straight through the
          // form.
          tabIndex={-1}
          aria-label={revealed ? "Hide password" : "Show password"}
          className="absolute inset-y-0 end-0 flex w-10 items-center justify-center
                     text-ink-faint transition-colors hover:text-ink"
        >
          <Icon className="h-4 w-4" />
        </button>
      </div>
      {hint && <span className="mt-1 block text-xs text-ink-faint">{hint}</span>}
    </label>
  );
}

export function Alert({
  tone,
  children,
}: {
  tone: "error" | "success" | "warn";
  children: ReactNode;
}) {
  const tones = {
    error: "bg-danger-soft text-danger",
    success: "bg-success-soft text-success",
    warn: "bg-warn-soft text-warn",
  };
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={`rounded-[var(--radius-control)] px-3 py-2 text-sm ${tones[tone]}`}
    >
      {children}
    </div>
  );
}

export function Badge({ children, tone }: { children: ReactNode; tone: string }) {
  const tones: Record<string, string> = {
    active: "bg-success-soft text-success",
    pending: "bg-warn-soft text-warn",
    suspended: "bg-danger-soft text-danger",
    admin: "bg-accent-soft text-accent",
    user: "bg-surface text-ink-muted",
  };
  return (
    <span
      className={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${
        tones[tone] ?? tones.user
      }`}
    >
      {children}
    </span>
  );
}

export function Card({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={`rounded-[var(--radius-card)] border border-border bg-raised ${className}`}
    >
      {children}
    </div>
  );
}

export function Spinner() {
  return (
    <div className="flex items-center justify-center py-16 text-ink-faint">
      <Loader2 className="h-5 w-5 animate-spin" />
    </div>
  );
}
