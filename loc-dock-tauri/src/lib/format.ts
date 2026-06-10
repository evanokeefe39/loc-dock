export function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toString();
}

export function fmtCost(n: number): string {
  return `$${n.toFixed(2)}`;
}

export function fmtLoc(n: number): string {
  return n.toLocaleString();
}
