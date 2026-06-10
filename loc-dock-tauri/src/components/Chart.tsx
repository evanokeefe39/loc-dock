import { useRef, useEffect } from "react";
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
    ctx.fillStyle = theme.chart_bg;
    ctx.fillRect(0, 0, w, h);

    if (!stats) {
      ctx.font = "11px 'Segoe UI'";
      ctx.fillStyle = theme.text_dim;
      ctx.textAlign = "center";
      ctx.fillText("loading…", w / 2, h / 2);
      return;
    }

    const labels = range === "day" ? stats.time_labels_day : stats.time_labels_week;

    if (mode === "loc") {
      const buckets = range === "day" ? stats.git_buckets_day : stats.git_buckets_week;
      drawLocChart(ctx, w, h, buckets, labels, theme);
    } else if (mode === "cost") {
      const buckets = range === "day" ? stats.cost_buckets_day : stats.cost_buckets_week;
      drawCostChart(ctx, w, h, buckets, labels, theme);
    } else {
      const buckets = range === "day" ? stats.token_buckets_day : stats.token_buckets_week;
      drawTokenChart(ctx, w, h, buckets, labels, theme);
    }
  }, [stats, mode, range, theme]);

  return <canvas ref={canvasRef} className="chart" />;
}
