import type { SurfaceMode, SurfaceTarget } from "../types/bridge";
import { useSurfaceSnapshot } from "./useSurfaceSnapshot";

/**
 * Subscribe to the current surface target for a given coarse mode.
 *
 * Returns null when the current mode does not match the requested mode so that
 * already-mounted surfaces can ignore retargets aimed at other surfaces.
 */
export function useSurfaceTarget(mode?: SurfaceMode): SurfaceTarget | null {
  const snapshot = useSurfaceSnapshot();

  if (mode !== undefined && snapshot.mode !== mode) {
    return null;
  }

  return snapshot.target;
}
