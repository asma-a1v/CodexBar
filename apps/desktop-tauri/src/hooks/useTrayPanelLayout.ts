import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import {
  getWorkAreaRect,
  reanchorTrayPanel,
  revealTrayPanelWindow,
} from "../lib/tauri";
import {
  decideTrayHeight,
  EMPTY_AUTOFIT_STATE,
  recordAutoFitCommit,
  type TrayAutoFitState,
} from "../lib/traySizing";

const TRAY_WIDTH = 328;
const TRAY_MAX_MEASURE_HEIGHT = 920;
const TRAY_OVERVIEW_MIN_HEIGHT = 200;
const TRAY_DETAIL_MIN_HEIGHT = 420;
const TRAY_DENSE_OVERVIEW_HEIGHT = 776;
// Leave a small viewport margin so borders and WebView2 pixel rounding cannot
// make the document alternate between overflowing and fitting by a few pixels.
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
  const lastSizeRef = useRef<{ width: number; height: number } | null>(null);
  const programmaticInFlightRef = useRef(0);
  const sizingStateRef = useRef<TrayAutoFitState>(EMPTY_AUTOFIT_STATE);

  // The tray flyout is content-sized only; it has no user-resizable mode.
  // Record the physical frame Windows actually applied so repeated layout
  // passes can distinguish a real content change from DPI rounding.
  const applySize = useCallback(async (size: LogicalSize): Promise<void> => {
    try {
      const win = getCurrentWindow();
      await win.setSize(size);
      const actual = await win.innerSize();
      lastSizeRef.current = { width: actual.width, height: actual.height };
    } catch {
      /* ignore */
    }
  }, []);

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
    const observer = new ResizeObserver(() => {
      // Measuring temporarily removes the surface/body constraints, which
      // resizes the observed surface. Do not feed that programmatic change
      // back into another pass or the capped flyout flashes between its
      // measured and committed layouts forever.
      if (
        layoutReadyRef.current &&
        programmaticInFlightRef.current > 0
      ) {
        return;
      }
      requestLayout();
    });
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
      const html = document.documentElement;
      const pageBody = document.body;
      const workArea = await getWorkAreaRect().catch(() => null);
      const maxHeight = Math.max(
        minHeight,
        Math.min(
          TRAY_MAX_MEASURE_HEIGHT,
          (workArea?.height ?? TRAY_MAX_MEASURE_HEIGHT) - 16,
        ),
      );

      const body = surface.querySelector<HTMLElement>(".menu-surface__body");
      const stack = surface.querySelector<HTMLElement>(".menu-stack");
      const previous = {
        htmlOverflow: html.style.overflow,
        bodyOverflow: pageBody.style.overflow,
        bodyMinHeight: pageBody.style.minHeight,
        surfaceMinHeight: surface.style.minHeight,
        surfaceHeight: surface.style.height,
        surfaceMaxHeight: surface.style.maxHeight,
        surfaceOverflow: surface.style.overflow,
        bodyInnerOverflow: body?.style.overflow,
        bodyFlex: body?.style.flex,
        stackOverflow: stack?.style.overflow,
      };
      let committedHeight = false;

      html.style.overflow = "visible";
      pageBody.style.overflow = "visible";
      pageBody.style.minHeight = "0";
      surface.style.minHeight = "0";
      surface.style.height = "auto";
      surface.style.maxHeight = "none";
      surface.style.overflow = "visible";
      if (body) {
        body.style.overflow = "visible";
        body.style.flex = "0 0 auto";
      }
      if (stack) stack.style.overflow = "visible";

      const revealPanel = async () => {
        if (run !== resizeRunRef.current) return;
        layoutReadyRef.current = true;
        setLayoutReady(true);
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        if (run === resizeRunRef.current) {
          await Promise.resolve(revealTrayPanelWindow()).catch(() => {});
        }
      };

      // Keep ResizeObserver callbacks caused by this measurement pass from
      // scheduling another pass. The trailing delay absorbs callbacks that
      // WebView2 delivers shortly after the styles and window size settle.
      programmaticInFlightRef.current += 1;
      try {
        if (!layoutReadyRef.current) {
          sizingStateRef.current = recordAutoFitCommit(
            sizingStateRef.current,
            TRAY_WIDTH,
            minHeight,
            window.devicePixelRatio,
          );
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

        const decision = decideTrayHeight(
          {
            measuredHeight: contentHeight,
            expectedWidth: TRAY_WIDTH,
            minHeight,
            maxHeight,
            scaleFactor: window.devicePixelRatio,
            zoom: 1,
            lastAppliedPhysicalHeight: lastSizeRef.current?.height ?? null,
          },
          sizingStateRef.current,
        );
        sizingStateRef.current = decision.state;
        surface.style.maxHeight = `${decision.height}px`;
        committedHeight = true;

        if (decision.commit) {
          await applySize(new LogicalSize(TRAY_WIDTH, decision.height));
          await Promise.resolve(reanchorTrayPanel()).catch(() => {});
        }

        await revealPanel();
      } catch (error) {
        console.warn("CodexBar tray panel resize failed", error);
        void revealPanel();
      } finally {
        if (!committedHeight) {
          surface.style.maxHeight = previous.surfaceMaxHeight;
        }
        surface.style.minHeight = previous.surfaceMinHeight;
        surface.style.height = previous.surfaceHeight;
        surface.style.overflow = previous.surfaceOverflow;
        html.style.overflow = previous.htmlOverflow;
        pageBody.style.overflow = previous.bodyOverflow;
        pageBody.style.minHeight = previous.bodyMinHeight;
        if (body) {
          body.style.overflow = previous.bodyInnerOverflow ?? "";
          body.style.flex = previous.bodyFlex ?? "";
        }
        if (stack) stack.style.overflow = previous.stackOverflow ?? "";
        window.setTimeout(() => {
          programmaticInFlightRef.current = Math.max(
            0,
            programmaticInFlightRef.current - 1,
          );
        }, 200);
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
