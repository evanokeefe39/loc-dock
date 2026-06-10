use crate::types::{CostBreakdown, TokenTotals};

pub const INPUT_PRICE: f64 = 15.00;
pub const OUTPUT_PRICE: f64 = 75.00;
pub const CACHE_WRITE_PRICE: f64 = 18.75;
pub const CACHE_READ_PRICE: f64 = 1.50;

pub fn estimate_cost(t: &TokenTotals) -> f64 {
    (t.input_tokens as f64 / 1_000_000.0) * INPUT_PRICE
        + (t.output_tokens as f64 / 1_000_000.0) * OUTPUT_PRICE
        + (t.cache_creation_input_tokens as f64 / 1_000_000.0) * CACHE_WRITE_PRICE
        + (t.cache_read_input_tokens as f64 / 1_000_000.0) * CACHE_READ_PRICE
}

pub fn cost_breakdown_from_tokens(t: &TokenTotals) -> CostBreakdown {
    CostBreakdown {
        input: (t.input_tokens as f64 / 1_000_000.0) * INPUT_PRICE,
        output: (t.output_tokens as f64 / 1_000_000.0) * OUTPUT_PRICE,
        cache_write: (t.cache_creation_input_tokens as f64 / 1_000_000.0) * CACHE_WRITE_PRICE,
        cache_read: (t.cache_read_input_tokens as f64 / 1_000_000.0) * CACHE_READ_PRICE,
    }
}
