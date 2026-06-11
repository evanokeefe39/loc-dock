import { TokenTotals } from "../lib/types";
import { fmtTokens } from "../lib/format";

interface Props {
  tokens: TokenTotals | null;
}

export function BottomRow({ tokens }: Props) {
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
    </div>
  );
}
