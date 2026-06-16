import { useRef, useEffect, useCallback, useState } from "react";
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
  const statsRef = useRef(stats);
  const modeRef = useRef(mode);
  const rangeRef = useRef(range);
  const themeRef = useRef(theme);
  const [spinnerFrame, setSpinnerFrame] = useState(0);
  const spinnerFrameRef = useRef(0);

  statsRef.current = stats;
  modeRef.current = mode;
  rangeRef.current = range;
  themeRef.current = theme;
  spinnerFrameRef.current = spinnerFrame;

  const loading = !stats || !stats.ready;

  useEffect(() => {
    if (!loading) return;
    const id = setInterval(() => setSpinnerFrame(f => (f + 1) % SPINNER_FRAMES.length), 80);
    return () => clearInterval(id);
  }, [loading]);

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

    const s = statsRef.current;
    if (!s || !s.ready) {
      ctx.font = "12px 'Segoe UI'";
      ctx.fillStyle = themeRef.current.text_dim;
      ctx.textAlign = "center";
      ctx.fillText(`${SPINNER_FRAMES[spinnerFrameRef.current]} loading...`, w / 2, h / 2);
      return;
    }

    const r = rangeRef.current;
    const labels = r === "day" ? s.time_labels_day
      : r === "week" ? s.time_labels_week
      : r === "month" ? s.time_labels_month
      : s.time_labels_year;

    if (modeRef.current === "loc") {
      const buckets = r === "day" ? s.git_buckets_day
        : r === "week" ? s.git_buckets_week
        : r === "month" ? s.git_buckets_month
        : s.git_buckets_year;
      drawLocChart(ctx, w, h, buckets, labels, themeRef.current);
    } else if (modeRef.current === "cost") {
      const buckets = r === "day" ? s.cost_buckets_day
        : r === "week" ? s.cost_buckets_week
        : r === "month" ? s.cost_buckets_month
        : s.cost_buckets_year;
      drawCostChart(ctx, w, h, buckets, labels, themeRef.current);
    } else {
      const buckets = r === "day" ? s.token_buckets_day
        : r === "week" ? s.token_buckets_week
        : r === "month" ? s.token_buckets_month
        : s.token_buckets_year;
      drawTokenChart(ctx, w, h, buckets, labels, themeRef.current);
    }
  }, []);

  useEffect(() => {
    draw();
  }, [stats, mode, range, theme, draw, spinnerFrame]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const observer = new ResizeObserver(() => draw());
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [draw]);

  return <canvas ref={canvasRef} className="chart" />;
}
