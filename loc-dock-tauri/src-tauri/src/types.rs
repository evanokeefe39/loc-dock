use serde::{Deserialize, Serialize};

/// Time range variants for stats queries.
/// Replaces stringly-typed `&[&str]` with a typed enum.
#[derive(Debug, Clone, Copy, PartialEq)]
// ponytail: unused since switching to daily_aggregates. Keep for future manual triggers.
#[allow(dead_code)]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AllStats {
    pub ready: bool,
    pub day: RangeStats,
    pub week: RangeStats,
    pub month: RangeStats,
    pub year: RangeStats,
    pub git_buckets_day: Vec<(i64, i64)>,
    pub git_buckets_week: Vec<(i64, i64)>,
    pub git_buckets_month: Vec<(i64, i64)>,
    pub git_buckets_year: Vec<(i64, i64)>,
    pub cost_buckets_day: Vec<f64>,
    pub cost_buckets_week: Vec<f64>,
    pub cost_buckets_month: Vec<f64>,
    pub cost_buckets_year: Vec<f64>,
    pub token_buckets_day: Vec<(i64, i64, i64, i64)>,
    pub token_buckets_week: Vec<(i64, i64, i64, i64)>,
    pub token_buckets_month: Vec<(i64, i64, i64, i64)>,
    pub token_buckets_year: Vec<(i64, i64, i64, i64)>,
    pub time_labels_day: TimeLabels,
    pub time_labels_week: TimeLabels,
    pub time_labels_month: TimeLabels,
    pub time_labels_year: TimeLabels,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct RangeStats {
    pub loc_added: i64,
    pub loc_deleted: i64,
    pub cost_total: f64,
    pub cost_breakdown: CostBreakdown,
    pub tokens: TokenTotals,
    pub sessions_total: i64,
    pub sessions_active: i64,
    pub source_breakdown: Vec<SourceStats>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct SourceStats {
    pub source: String,
    pub sessions_total: i64,
    pub sessions_active: i64,
    pub tokens: TokenTotals,
    pub cost_total: f64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct CostBreakdown {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TokenTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TimeLabels {
    pub start: String,
    pub end: String,
    pub ticks: Vec<Tick>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Tick {
    pub frac: f64,
    pub label: String,
}
