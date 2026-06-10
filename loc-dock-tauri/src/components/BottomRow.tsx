import { useState, useEffect } from "react";
import { TokenTotals } from "../lib/types";
import { fmtTokens } from "../lib/format";

const SPINNER_FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

interface Props {
  tokens: TokenTotals | null;
  loading?: boolean;
}

export function BottomRow({ tokens, loading }: Props) {
  const [frame, setFrame] = useState(0);

  useEffect(() => {
    if (!loading) return;
    const id = setInterval(() => setFrame(f => (f + 1) % SPINNER_FRAMES.length), 80);
    return () => clearInterval(id);
  }, [loading]);

  const t = tokens;
  const total = t ? t.input_tokens + t.output_tokens + t.cache_creation_input_tokens + t.cache_read_input_tokens : 0;
  return (
    <div className="bottom-row">
      <span className="stat-label">IN</span>
      <span className="stat-value tok-input">{t ? fmtTokens(t.input_tokens) : "--"}</span>
      <span className="stat-label">OUT</span>
      <span className="stat-value tok-output">{t ? fmtTokens(t.output_tokens) : "--"}</span>
      <span className="stat-label">CW</span>
      <span className="stat-value tok-cache-write">{t ? fmtTokens(t.cache_creation_input_tokens) : "--"}</span>
      <span className="stat-label">CR</span>
      <span className="stat-value tok-cache-read">{t ? fmtTokens(t.cache_read_input_tokens) : "--"}</span>
      <span className="stat-label">TOT</span>
      <span className="stat-value">{t ? fmtTokens(total) : "--"}</span>
      <div className="spacer" />
      {loading && <span className="spinner">{SPINNER_FRAMES[frame]}</span>}
    </div>
  );
}
