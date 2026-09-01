import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DisplayStatus, PeerStatus } from "../ipc/types";

interface Block {
  key: string;
  label: string;
  local: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
}

interface SnapOutcome {
  block: { x: number; y: number; width: number; height: number };
  snapped: boolean;
  overlapping: boolean;
}

/**
 * Drag computers around to say where they sit relative to each other.
 *
 * Every geometric decision — where a dropped block comes to rest, whether the
 * arrangement is valid — is made by `mb-topology` through the `snap_block`
 * command. This component only tracks the pointer.
 *
 * That split is deliberate. A one-point gap between two screens is invisible at
 * this scale and completely breaks crossing: the cursor reaches the edge, finds
 * nothing beyond it, and stops. Deciding adjacency in TypeScript, by eye, is how
 * that bug ships.
 */
export function LayoutEditor({
  displays,
  peers,
}: {
  displays: DisplayStatus[];
  peers: PeerStatus[];
}) {
  const [blocks, setBlocks] = useState<Block[]>([]);
  const [dragging, setDragging] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const canvas = useRef<HTMLDivElement>(null);
  const grab = useRef({ dx: 0, dy: 0 });

  // Rebuild whenever the machines or their screens change. The editor is a view
  // of real state, not a separate arrangement that can drift out of step.
  useEffect(() => {
    const local: Block[] = displays.map((d, index) => ({
      key: `local-${d.id}`,
      label: d.name ?? `This computer${displays.length > 1 ? ` (${index + 1})` : ""}`,
      local: true,
      x: d.x,
      y: d.y,
      width: d.width,
      height: d.height,
    }));

    const localWidth = local.reduce((max, b) => Math.max(max, b.x + b.width), 0);
    const remote: Block[] = peers.map((peer, index) => ({
      key: `peer-${peer.id_short}`,
      label: peer.name,
      local: false,
      x: localWidth + index * 1920,
      y: 0,
      width: 1920,
      height: 1080,
    }));

    setBlocks([...local, ...remote]);
  }, [displays, peers]);

  const scale = (() => {
    if (blocks.length === 0) return 1;
    const maxX = Math.max(...blocks.map((b) => b.x + b.width));
    const maxY = Math.max(...blocks.map((b) => b.y + b.height));
    const minX = Math.min(...blocks.map((b) => b.x));
    const minY = Math.min(...blocks.map((b) => b.y));
    const span = Math.max(maxX - minX, 1);
    const height = Math.max(maxY - minY, 1);
    return Math.min(460 / span, 220 / height, 0.2);
  })();

  const origin = blocks.length
    ? {
        x: Math.min(...blocks.map((b) => b.x)),
        y: Math.min(...blocks.map((b) => b.y)),
      }
    : { x: 0, y: 0 };

  const onPointerDown = (key: string, event: React.PointerEvent) => {
    const block = blocks.find((b) => b.key === key);
    if (!block) return;
    const rect = canvas.current?.getBoundingClientRect();
    if (!rect) return;
    grab.current = {
      dx: (event.clientX - rect.left) / scale + origin.x - block.x,
      dy: (event.clientY - rect.top) / scale + origin.y - block.y,
    };
    setDragging(key);
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: React.PointerEvent) => {
    if (!dragging) return;
    const rect = canvas.current?.getBoundingClientRect();
    if (!rect) return;
    const x = (event.clientX - rect.left) / scale + origin.x - grab.current.dx;
    const y = (event.clientY - rect.top) / scale + origin.y - grab.current.dy;
    setBlocks((current) =>
      current.map((b) => (b.key === dragging ? { ...b, x, y } : b)),
    );
  };

  const onPointerUp = useCallback(() => {
    if (!dragging) return;
    const key = dragging;
    setDragging(null);

    setBlocks((current) => {
      const moved = current.find((b) => b.key === key);
      if (!moved) return current;
      const others = current.filter((b) => b.key !== key);

      // The Rust side decides. This only reports where the pointer was.
      void invoke<SnapOutcome>("snap_block", {
        dragged: { x: moved.x, y: moved.y, width: moved.width, height: moved.height },
        others: others.map((b) => ({ x: b.x, y: b.y, width: b.width, height: b.height })),
      })
        .then((outcome) => {
          setInvalid(outcome.overlapping);
          setBlocks((latest) =>
            latest.map((b) =>
              b.key === key ? { ...b, x: outcome.block.x, y: outcome.block.y } : b,
            ),
          );
        })
        .catch(() => setInvalid(true));

      return current;
    });
  }, [dragging]);

  if (blocks.length === 0) {
    return (
      <p className="text-sm text-[var(--color-ink-muted)]">
        No screens to arrange yet.
      </p>
    );
  }

  const spanX = Math.max(...blocks.map((b) => b.x + b.width)) - origin.x;
  const spanY = Math.max(...blocks.map((b) => b.y + b.height)) - origin.y;

  return (
    <div>
      <div
        ref={canvas}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        className="relative mx-auto touch-none"
        style={{ width: spanX * scale, height: spanY * scale, minHeight: 120 }}
      >
        {blocks.map((block) => (
          <div
            key={block.key}
            onPointerDown={(e) => onPointerDown(block.key, e)}
            className={`absolute flex cursor-grab flex-col items-center justify-center rounded-md border-2 px-1 text-center select-none ${
              dragging === block.key ? "z-10 cursor-grabbing shadow-lg" : ""
            } ${
              block.local
                ? "border-blue-500 bg-blue-500/10"
                : "border-emerald-500 bg-emerald-500/10"
            }`}
            style={{
              left: (block.x - origin.x) * scale,
              top: (block.y - origin.y) * scale,
              width: block.width * scale,
              height: block.height * scale,
            }}
          >
            <span className="truncate text-[11px] font-medium">{block.label}</span>
            <span className="text-[10px] tabular-nums text-[var(--color-ink-muted)]">
              {Math.round(block.width)} × {Math.round(block.height)}
            </span>
          </div>
        ))}
      </div>

      <p
        className={`mt-3 text-xs ${
          invalid ? "text-red-500" : "text-[var(--color-ink-muted)]"
        }`}
      >
        {invalid
          ? "These screens overlap. Move them apart before saving — the pointer cannot cross an overlapping edge."
          : "Drag a computer to say where it sits. Screens snap together so the pointer can cross between them."}
      </p>
    </div>
  );
}
