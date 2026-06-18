import { useRef, useEffect, useState } from "react";
import { AllStats, ChartMode, TimeRange, Theme } from "../lib/types";
import { drawLocChart, drawCostChart, drawTokenChart } from "../lib/chart";

const SPINNER_FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

interface Props {
  stats: AllStats | null;
  mode: ChartMode;
  range: TimeRange;
  theme: Theme;
}

export function Chart({ stats, mode, range, theme }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [spinnerFrame, setSpinnerFrame] = useState(0);
  const [resizeTick, setResizeTick] = useState(0);
  const loading = !stats || !stats.ready;

  // Spinner animation while loading
  useEffect(() => {
    if (!loading) return;
    const id = setInterval(() => setSpinnerFrame(f => (f + 1) % SPINNER_FRAMES.length), 80);
    return () => clearInterval(id);
  }, [loading]);

  // Draw the chart
  useEffect(() => {
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

    if (!stats || !stats.ready) {
      ctx.font = "12px 'Segoe UI'";
      ctx.fillStyle = theme.text_dim;
      ctx.textAlign = "center";
      ctx.fillText(`${SPINNER_FRAMES[spinnerFrame]} loading...`, w / 2, h / 2);
      return;
    }

    const labels = range === "day" ? stats.time_labels_day
      : range === "week" ? stats.time_labels_week
      : range === "month" ? stats.time_labels_month
      : stats.time_labels_year;

    if (mode === "loc") {
      const buckets = range === "day" ? stats.git_buckets_day
        : range === "week" ? stats.git_buckets_week
        : range === "month" ? stats.git_buckets_month
        : stats.git_buckets_year;
      drawLocChart(ctx, w, h, buckets, labels, theme);
    } else if (mode === "cost") {
      const buckets = range === "day" ? stats.cost_buckets_day
        : range === "week" ? stats.cost_buckets_week
        : range === "month" ? stats.cost_buckets_month
        : stats.cost_buckets_year;
      drawCostChart(ctx, w, h, buckets, labels, theme);
    } else {
      const buckets = range === "day" ? stats.token_buckets_day
        : range === "week" ? stats.token_buckets_week
        : range === "month" ? stats.token_buckets_month
        : stats.token_buckets_year;
      drawTokenChart(ctx, w, h, buckets, labels, theme);
    }
  }, [stats, mode, range, theme, spinnerFrame, resizeTick]);

  // Resize observer — triggers redraw via resizeTick counter
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() => setResizeTick(t => t + 1));
    observer.observe(canvas);
    return () => observer.disconnect();
  }, []);

  return <canvas ref={canvasRef} className="chart" />;
}
