import type { DisplayStatus } from "../ipc/types";

/**
 * Draws the attached displays to scale.
 *
 * A preview of the topology editor arriving in milestone 7. It renders only the
 * local machine's screens, which is genuinely all this build knows about — it is
 * not a mock of the multi-device layout.
 */
export function DisplayMap({ displays }: { displays: DisplayStatus[] }) {
  if (displays.length === 0) return null;

  const minX = Math.min(...displays.map((d) => d.x));
  const minY = Math.min(...displays.map((d) => d.y));
  const maxX = Math.max(...displays.map((d) => d.x + d.width));
  const maxY = Math.max(...displays.map((d) => d.y + d.height));
  const spanX = maxX - minX;
  const spanY = maxY - minY;
  if (spanX <= 0 || spanY <= 0) return null;

  // Fit the arrangement into a fixed box while preserving aspect ratio.
  const BOX_W = 460;
  const BOX_H = 200;
  const scale = Math.min(BOX_W / spanX, BOX_H / spanY);

  return (
    <div
      className="relative mx-auto"
      style={{ width: spanX * scale, height: spanY * scale }}
    >
      {displays.map((d) => (
        <div
          key={d.id}
          className={`absolute flex flex-col items-center justify-center rounded-md border-2 text-center ${
            d.is_primary
              ? "border-blue-500 bg-blue-500/10"
              : "border-[var(--color-line)] bg-[var(--color-line)]/20"
          }`}
          style={{
            left: (d.x - minX) * scale,
            top: (d.y - minY) * scale,
            width: d.width * scale,
            height: d.height * scale,
          }}
          title={d.name ?? `Display ${d.id}`}
        >
          <span className="px-1 text-[11px] font-medium tabular-nums">
            {Math.round(d.width)} × {Math.round(d.height)}
          </span>
          <span className="text-[10px] text-[var(--color-ink-muted)]">
            {d.scale.toFixed(2)}× {d.is_primary ? "· primary" : ""}
          </span>
        </div>
      ))}
    </div>
  );
}
