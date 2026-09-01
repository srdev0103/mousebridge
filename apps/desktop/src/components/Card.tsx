import type { ReactNode } from "react";

export function Card({
  title,
  action,
  children,
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="rounded-xl border border-[var(--color-line)] bg-[var(--color-surface-muted)] p-5">
      <header className="mb-4 flex items-center justify-between gap-3">
        <h2 className="text-sm font-semibold tracking-wide text-[var(--color-ink-muted)] uppercase">
          {title}
        </h2>
        {action}
      </header>
      {children}
    </section>
  );
}

export function Row({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 py-1.5 text-sm">
      <span className="text-[var(--color-ink-muted)]">{label}</span>
      <span className="text-right font-medium tabular-nums">{value}</span>
    </div>
  );
}

export function Button({
  children,
  onClick,
  variant = "secondary",
}: {
  children: ReactNode;
  onClick: () => void;
  variant?: "primary" | "secondary";
}) {
  const styles =
    variant === "primary"
      ? "bg-blue-600 text-white hover:bg-blue-500"
      : "border border-[var(--color-line)] hover:bg-[var(--color-line)]/40";
  return (
    <button
      type="button"
      onClick={onClick}
      className={`cursor-pointer rounded-lg px-3 py-1.5 text-sm font-medium transition-colors ${styles}`}
    >
      {children}
    </button>
  );
}
