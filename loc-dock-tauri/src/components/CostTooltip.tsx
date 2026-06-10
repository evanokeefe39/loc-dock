import { CostBreakdown } from "../lib/types";

interface Props {
  breakdown: CostBreakdown | null;
  visible: boolean;
}

export function CostTooltip({ breakdown, visible }: Props) {
  if (!visible || !breakdown) return null;
  const total = breakdown.input + breakdown.output + breakdown.cache_write + breakdown.cache_read;
  return (
    <div className="cost-tooltip">
      <pre>{`IN   $${breakdown.input.toFixed(2)}  @$15/MTok
OUT  $${breakdown.output.toFixed(2)}  @$75/MTok
CW   $${breakdown.cache_write.toFixed(2)}  @$18.75/MTok
CR   $${breakdown.cache_read.toFixed(2)}  @$1.50/MTok
─────────────────
TOT  $${total.toFixed(2)}`}</pre>
    </div>
  );
}
