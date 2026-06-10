use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AllStats {
    pub day: RangeStats,
    pub week: RangeStats,
    pub git_buckets_day: Vec<(i64, i64)>,
    pub git_buckets_week: Vec<(i64, i64)>,
    pub cost_buckets_day: Vec<f64>,
    pub cost_buckets_week: Vec<f64>,
    pub token_buckets_day: Vec<(i64, i64, i64, i64)>,
    pub token_buckets_week: Vec<(i64, i64, i64, i64)>,
    pub time_labels_day: TimeLabels,
    pub time_labels_week: TimeLabels,
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
