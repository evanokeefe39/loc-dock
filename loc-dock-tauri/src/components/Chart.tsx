import { useRef, useEffect, useCallback } from "react";
import { AllStats, ChartMode, TimeRange, Theme } from "../lib/types";
import { drawLocChart, drawCostChart, drawTokenChart } from "../lib/chart";

interface Props {
  stats: AllStats | null;
  mode: ChartMode;
  range: TimeRange;
  theme: Theme;
}

export function Chart({ stats, mode, range, theme }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const statsRef = useRef(stats);
  const modeRef = useRef(mode);
  const rangeRef = useRef(range);
  const themeRef = useRef(theme);

  statsRef.current = stats;
  modeRef.current = mode;
  rangeRef.current = range;
  themeRef.current = theme;

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    ctx.scale(dpr, dpr);
    const w = rect.width;
    const h = rect.height;

    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = themeRef.current.chart_bg;
    ctx.fillRect(0, 0, w, h);

    const s = statsRef.current;
    if (!s) {
      ctx.font = "12px 'Segoe UI'";
      ctx.fillStyle = themeRef.current.text_dim;
      ctx.textAlign = "center";
      ctx.fillText("loading…", w / 2, h / 2);
      return;
    }

    const labels = rangeRef.current === "day" ? s.time_labels_day : s.time_labels_week;

    if (modeRef.current === "loc") {
      const buckets = rangeRef.current === "day" ? s.git_buckets_day : s.git_buckets_week;
      drawLocChart(ctx, w, h, buckets, labels, themeRef.current);
    } else if (modeRef.current === "cost") {
      const buckets = rangeRef.current === "day" ? s.cost_buckets_day : s.cost_buckets_week;
      drawCostChart(ctx, w, h, buckets, labels, themeRef.current);
    } else {
      const buckets = rangeRef.current === "day" ? s.token_buckets_day : s.token_buckets_week;
      drawTokenChart(ctx, w, h, buckets, labels, themeRef.current);
    }
  }, []);

  useEffect(() => {
    draw();
  }, [stats, mode, range, theme, draw]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() => draw());
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [draw]);

  return <canvas ref={canvasRef} className="chart" />;
}
