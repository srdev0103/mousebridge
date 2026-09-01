import { useCallback, useEffect, useRef, useState } from "react";
import { engineSnapshot } from "./engine";
import type { EngineSnapshot } from "./engine";

/**
 * Polled faster than the configuration snapshot.
 *
 * Discovery and pairing move on their own — a machine appears, a code shows up —
 * so a slow poll makes the interface feel broken. One second is quick enough
 * that a newly launched peer appears while the user is still looking, and the
 * query is a few in-memory reads.
 */
const POLL_MS = 1000;

export function useEngine() {
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await engineSnapshot();
      if (alive.current) setSnapshot(next);
    } catch {
      // The engine failed to start. The dashboard says so; there is nothing to
      // retry here, and a console error every second would help nobody.
      if (alive.current) setSnapshot(null);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
    };
  }, [refresh]);

  return { snapshot, refresh };
}
