import { TimeLabels, Theme } from "./types";
import { fmtTokens } from "./format";

const TIME_PAD = 28;

function drawTimeLabels(
  ctx: CanvasRenderingContext2D,
  w: number,
  axisY: number,
  labels: TimeLabels,
  theme: Theme,
) {
  ctx.font = "10px 'Segoe UI'";
  ctx.fillStyle = theme.text_dim;
  ctx.textAlign = "left";
  ctx.fillText(labels.start, 4, axisY + 12);

  for (const tick of labels.ticks) {
    const x = TIME_PAD + tick.frac * (w - 2 * TIME_PAD);
    if (x <= TIME_PAD + 12) continue;
    ctx.strokeStyle = theme.axis;
    ctx.beginPath();
    ctx.moveTo(x, axisY);
    ctx.lineTo(x, axisY + 3);
    ctx.stroke();
    ctx.fillStyle = theme.text_dim;
    ctx.textAlign = "center";
    ctx.fillText(tick.label, x, axisY + 12);
  }
}

function drawChrome(
  ctx: CanvasRenderingContext2D,
  label: string,
  theme: Theme,
) {
  if (!label) return;
  ctx.font = "9px 'Segoe UI'";
  ctx.fillStyle = theme.text_dim;
  ctx.textAlign = "left";
  ctx.fillText(label, 3, 10);
}

export function drawLocChart(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  buckets: [number, number][],
  labels: TimeLabels,
  theme: Theme,
) {
  const bottom = h - 18;

  const hasData = buckets.some(([a, d]) => a > 0 || d > 0);
  if (!hasData) {
    ctx.font = "11px 'Segoe UI'";
    ctx.fillStyle = theme.text_dim;
    ctx.textAlign = "center";
    ctx.fillText("no commits yet", w / 2, h / 2);
    return;
  }

  // Draw axis line
  ctx.strokeStyle = theme.axis;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, bottom);
  ctx.lineTo(w, bottom);
  ctx.stroke();

  drawTimeLabels(ctx, w, bottom, labels, theme);

  const barLeft = TIME_PAD;
  const barZone = w - 2 * TIME_PAD;
  if (barZone < 10) return;

  const maxVal = Math.max(...buckets.map(([a, d]) => a + d), 1);
  drawChrome(ctx, maxVal.toLocaleString(), theme);

  const barW = Math.max(barZone / buckets.length, 1);
  const usableH = bottom - 4;

  for (let i = 0; i < buckets.length; i++) {
    const [a, d] = buckets[i];
    if (a === 0 && d === 0) continue;

    const x0 = barLeft + i * barW;
    const x1 = x0 + barW - 1;
    const total = a + d;
    const totalH = Math.max((total / maxVal) * usableH, 1);
    const greenH = total > 0 ? (a / total) * totalH : 0;
    const redH = totalH - greenH;

    let y = bottom;

    // Green (additions) on bottom
    if (greenH > 0) {
      ctx.fillStyle = theme.loc_add;
      ctx.fillRect(x0, y - greenH, x1 - x0, greenH);
      y -= greenH;
    }

    // Red (deletions) stacked on top
    if (redH > 0) {
      ctx.fillStyle = theme.loc_del;
      ctx.fillRect(x0, y - redH, x1 - x0, redH);
    }
  }
}

export function drawCostChart(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  buckets: number[],
  labels: TimeLabels,
  theme: Theme,
) {
  const bottom = h - 18;

  // Always draw axis and time labels for cost chart
  ctx.strokeStyle = theme.axis;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, bottom);
  ctx.lineTo(w, bottom);
  ctx.stroke();

  drawTimeLabels(ctx, w, bottom, labels, theme);

  const hasData = buckets.some((v) => v > 0);
  if (!hasData) {
    drawChrome(ctx, "", theme);
    ctx.font = "11px 'Segoe UI'";
    ctx.fillStyle = theme.text_dim;
    ctx.textAlign = "center";
    ctx.fillText("no cost data", w / 2, h / 2);
    return;
  }

  const barLeft = TIME_PAD;
  const barZone = w - 2 * TIME_PAD;
  if (barZone < 10) return;

  const maxVal = Math.max(...buckets, 1);
  drawChrome(ctx, `$${maxVal.toFixed(2)}`, theme);

  const barW = Math.max(barZone / buckets.length, 1);
  const usableH = bottom - 4;

  for (let i = 0; i < buckets.length; i++) {
    const v = buckets[i];
    if (v <= 0) continue;

    const x0 = barLeft + i * barW;
    const x1 = x0 + barW - 1;
    const bh = Math.max((v / maxVal) * usableH, 1);

    ctx.fillStyle = theme.cost;
    ctx.fillRect(x0, bottom - bh, x1 - x0, bh);
  }
}

export function drawTokenChart(
  ctx: CanvasRenderingContext2D,
  w: number,
  h: number,
  buckets: [number, number, number, number][],
  labels: TimeLabels,
  theme: Theme,
) {
  const bottom = h - 18;

  // Always draw axis and time labels for token chart
  ctx.strokeStyle = theme.axis;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, bottom);
  ctx.lineTo(w, bottom);
  ctx.stroke();

  drawTimeLabels(ctx, w, bottom, labels, theme);

  const hasData = buckets.some(([inp, out, cw, cr]) => inp + out + cw + cr > 0);
  if (!hasData) {
    drawChrome(ctx, "", theme);
    ctx.font = "11px 'Segoe UI'";
    ctx.fillStyle = theme.text_dim;
    ctx.textAlign = "center";
    ctx.fillText("no token data", w / 2, h / 2);
    return;
  }

  const barLeft = TIME_PAD;
  const barZone = w - 2 * TIME_PAD;
  if (barZone < 10) return;

  const maxVal = Math.max(
    ...buckets.map(([inp, out, cw, cr]) => inp + out + cw + cr),
    1,
  );
  drawChrome(ctx, fmtTokens(maxVal), theme);

  const barW = Math.max(barZone / buckets.length, 1);
  const usableH = bottom - 4;

  // Stack order bottom-to-top: cache_read, cache_write, output, input
  for (let i = 0; i < buckets.length; i++) {
    const [inp, out, cw, cr] = buckets[i];
    const x0 = barLeft + i * barW;
    const x1 = x0 + barW - 1;
    let y = bottom;

    const layers: [number, string][] = [
      [cr, theme.tok_cache_read],
      [cw, theme.tok_cache_write],
      [out, theme.tok_output],
      [inp, theme.tok_input],
    ];

    for (const [val, color] of layers) {
      if (val <= 0) continue;
      const bh = Math.max((val / maxVal) * usableH, 1);
      ctx.fillStyle = color;
      ctx.fillRect(x0, y - bh, x1 - x0, bh);
      y -= bh;
    }
  }
}
