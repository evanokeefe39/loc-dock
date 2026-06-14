export interface AllStats {
  ready: boolean;
  day: RangeStats;
  week: RangeStats;
  git_buckets_day: [number, number][];
  git_buckets_week: [number, number][];
  cost_buckets_day: number[];
  cost_buckets_week: number[];
  token_buckets_day: [number, number, number, number][];
  token_buckets_week: [number, number, number, number][];
  time_labels_day: TimeLabels;
  time_labels_week: TimeLabels;
}

export interface RangeStats {
  loc_added: number;
  loc_deleted: number;
  cost_total: number;
  cost_breakdown: CostBreakdown;
  tokens: TokenTotals;
  sessions_total: number;
  sessions_active: number;
}

export interface CostBreakdown {
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
}

export interface TokenTotals {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
}

export interface TimeLabels {
  start: string;
  end: string;
  ticks: Tick[];
}

export interface Tick {
  frac: number;
  label: string;
}

export interface Theme {
  alpha: number;
  bg: string;
  chart_bg: string;
  tooltip_bg: string;
  text: string;
  text_dim: string;
  axis: string;
  loc_add: string;
  loc_del: string;
  cost: string;
  sessions: string;
  tok_input: string;
  tok_output: string;
  tok_cache_write: string;
  tok_cache_read: string;
}

export type TimeRange = "day" | "week";
export type ChartMode = "loc" | "cost" | "tokens";

export interface RepoSummary {
  name: string;
  commits: number;
  highlights: string[];
}

export interface SummaryData {
  repos: RepoSummary[];
  day_repos: number;
  day_commits: number;
  loading: boolean;
  no_api_key: boolean;
}
