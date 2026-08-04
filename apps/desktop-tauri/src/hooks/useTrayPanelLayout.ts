import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import {
  getWorkAreaRect,
  reanchorTrayPanel,
  revealTrayPanelWindow,
} from "../lib/tauri";

const TRAY_WIDTH = 328;
const TRAY_MAX_MEASURE_HEIGHT = 920;
const TRAY_OVERVIEW_MIN_HEIGHT = 200;
const TRAY_DETAIL_MIN_HEIGHT = 420;
const TRAY_DENSE_OVERVIEW_HEIGHT = 776;
// Leave a small viewport margin so fractional CSS zoom, borders, and WebView2
// pixel rounding cannot make the document alternate between overflowing and
// fitting by one or two pixels.
const TRAY_HEIGHT_SAFETY_PX = 10;

export interface TrayPanelLayoutOptions {
  canMeasure: boolean;
  denseOverview: boolean;
  detailMode: boolean;
  layoutKey: string;
}

export interface TrayPanelLayout {
  layoutReady: boolean;
  requestLayout: () => void;
}

export function useTrayPanelLayout({
  canMeasure,
  denseOverview,
  detailMode,
  layoutKey,
}: TrayPanelLayoutOptions): TrayPanelLayout {
  const [layoutReady, setLayoutReady] = useState(false);
  const [layoutRevision, setLayoutRevision] = useState(0);
  const layoutReadyRef = useRef(false);
  const resizeRunRef = useRef(0);
  const layoutTimerRef = useRef<number | undefined>(undefined);
  // Track the last logical target so content-only refreshes do not resize or
  // re-anchor an already-correct flyout.
  const autoFitLogicalRef = useRef<{ width: number; height: number } | null>(
    null,
  );

  // The tray flyout is content-sized only; it has no user-resizable mode.
  const applySize = useCallback(
    async (size: LogicalSize): Promise<void> => {
      try {
        await getCurrentWindow().setSize(size);
      } catch {
        /* ignore */
      }
    },
    [],
  );

  const requestLayout = useCallback(() => {
    if (layoutTimerRef.current !== undefined) {
      window.clearTimeout(layoutTimerRef.current);
    }
    layoutTimerRef.current = window.setTimeout(() => {
      setLayoutRevision((current) => current + 1);
    }, layoutReadyRef.current ? 100 : 16);
  }, []);

  useEffect(() => {
    requestLayout();
  }, [layoutKey, requestLayout]);

  useEffect(() => {
    const surface = document.querySelector<HTMLElement>(".menu-surface--tray");
    if (!surface || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => requestLayout());
    observer.observe(surface);
    return () => observer.disconnect();
  }, [requestLayout]);

  useEffect(() => {
    return () => {
      if (layoutTimerRef.current !== undefined) {
        window.clearTimeout(layoutTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!canMeasure) return;

    const minHeight = detailMode
      ? TRAY_DETAIL_MIN_HEIGHT
      : denseOverview
        ? TRAY_DENSE_OVERVIEW_HEIGHT
        : TRAY_OVERVIEW_MIN_HEIGHT;

    const resize = async () => {
      const run = ++resizeRunRef.current;
      const surface = document.querySelector<HTMLElement>(".menu-surface--tray");
      if (!surface) return;
      const workArea = await getWorkAreaRect().catch(() => null);
      const maxHeight = Math.max(
        minHeight,
        Math.min(
          TRAY_MAX_MEASURE_HEIGHT,
          (workArea?.height ?? TRAY_MAX_MEASURE_HEIGHT) - 16,
        ),
      );

      const body = surface.querySelector<HTMLElement>(".menu-surface__body");

      const revealPanel = async () => {
        if (run !== resizeRunRef.current) return;
        layoutReadyRef.current = true;
        setLayoutReady(true);
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        if (run === resizeRunRef.current) {
          await Promise.resolve(revealTrayPanelWindow()).catch(() => {});
        }
      };

      try {
        if (!layoutReadyRef.current) {
          autoFitLogicalRef.current = { width: TRAY_WIDTH, height: minHeight };
          await applySize(new LogicalSize(TRAY_WIDTH, minHeight));
        }

        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

        if (run !== resizeRunRef.current) return;

        const surfaceRect = surface.getBoundingClientRect();
        let contentHeight = Math.max(surface.scrollHeight, surfaceRect.height);
        let maxBottom = surfaceRect.top + contentHeight;
        const bodyRect = body?.getBoundingClientRect();
        if (bodyRect && bodyRect.height > 0 && bodyRect.bottom > maxBottom) {
          maxBottom = bodyRect.bottom;
        }
        const footer = surface.querySelector<HTMLElement>(".menu-surface__footer");
        const footerRect = footer?.getBoundingClientRect();
        if (footerRect && footerRect.height > 0 && footerRect.bottom > maxBottom) {
          maxBottom = footerRect.bottom;
        }
        contentHeight =
          Math.ceil(maxBottom - surfaceRect.top) + TRAY_HEIGHT_SAFETY_PX;

        const height = Math.min(Math.max(contentHeight, minHeight), maxHeight);

        const previousSize = autoFitLogicalRef.current;
        const shouldResize =
          previousSize === null ||
          previousSize.width !== TRAY_WIDTH ||
          Math.abs(previousSize.height - height) > 2;
        if (shouldResize) {
          autoFitLogicalRef.current = { width: TRAY_WIDTH, height };
          await applySize(new LogicalSize(TRAY_WIDTH, height));
          await Promise.resolve(reanchorTrayPanel()).catch(() => {});
        }

        await revealPanel();
      } catch (error) {
        console.warn("CodexBar tray panel resize failed", error);
        void revealPanel();
      }
    };

    const timer = window.setTimeout(
      () => void resize(),
      layoutReadyRef.current ? 25 : 0,
    );

    return () => {
      window.clearTimeout(timer);
      resizeRunRef.current += 1;
    };
  }, [canMeasure, denseOverview, detailMode, layoutRevision, applySize]);

  return { layoutReady, requestLayout };
}
