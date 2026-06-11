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
