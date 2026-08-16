export interface AllStats {
  ready: boolean;
  day: RangeStats;
  week: RangeStats;
  month: RangeStats;
  year: RangeStats;
  git_buckets_day: [number, number][];
  git_buckets_week: [number, number][];
  git_buckets_month: [number, number][];
  git_buckets_year: [number, number][];
  cost_buckets_day: number[];
  cost_buckets_week: number[];
  cost_buckets_month: number[];
  cost_buckets_year: number[];
  token_buckets_day: [number, number, number, number][];
  token_buckets_week: [number, number, number, number][];
  token_buckets_month: [number, number, number, number][];
  token_buckets_year: [number, number, number, number][];
  time_labels_day: TimeLabels;
  time_labels_week: TimeLabels;
  time_labels_month: TimeLabels;
  time_labels_year: TimeLabels;
}

export interface RangeStats {
  loc_added: number;
  loc_deleted: number;
  cost_total: number;
  cost_breakdown: CostBreakdown;
  tokens: TokenTotals;
  sessions_total: number;
  sessions_active: number;
  source_breakdown: SourceStats[];
}

export interface SourceStats {
  source: string;
  sessions_total: number;
  sessions_active: number;
  tokens: TokenTotals;
  cost_total: number;
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

export type TimeRange = "day" | "week" | "month" | "year";
export type ChartMode = "loc" | "cost" | "tokens";
export type ConnectionStatus = "idle" | "testing" | "ok" | "failed";

export interface RepoSummary {
  name: string;
  commits: number;
  prs: string[];
  highlights: string[];
  /** True when a cached summary exists (even an explicit empty `[]`). */
  summarized: boolean;
}

export interface SummaryData {
  day_repos: RepoSummary[];
  day_repo_count: number;
  day_commits: number;
  day_prs: number;
  week_repos: RepoSummary[];
  week_repo_count: number;
  week_commits: number;
  week_prs: number;
  loading: boolean;
  no_api_key: boolean;
}
