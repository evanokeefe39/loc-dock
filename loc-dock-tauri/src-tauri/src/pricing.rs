use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Single model pricing entry from LiteLLM's model_prices_and_context_window.json.
///
/// LiteLLM stores prices **per token** (e.g. `0.000003` = $3 per million tokens).
/// The [`ModelCost`] returned by [`Pricing::get_per_million`] converts to
/// per-million-token for compatibility with the existing SQL token templating.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelPricing {
    #[serde(default)]
    pub input_cost_per_token: f64,
    #[serde(default)]
    pub output_cost_per_token: f64,
    #[serde(default)]
    pub cache_read_input_token_cost: f64,
    #[serde(default)]
    pub cache_creation_input_token_cost: f64,
}

impl Default for ModelPricing {
    fn default() -> Self {
        Self {
            // gpt-4o-mini per-token pricing (~$0.15/$0.60 per MTok)
            input_cost_per_token: 0.00000015,
            output_cost_per_token: 0.0000006,
            cache_read_input_token_cost: 0.000000075,
            cache_creation_input_token_cost: 0.00000015,
        }
    }
}

/// Pricing loaded from LiteLLM's model_prices_and_context_window.json.
///
/// Load chain (first match wins):
///   1. User override (settings `model_pricing_path`)
///   2. Bundled resource (shipped via CI/CD to `resources/pricing/litellm.json`)
///   3. Hardcoded defaults (gpt-4o-mini pricing)
///
/// The old flat `pricing.yaml` is **gone** — all pricing comes from LiteLLM's
/// community-maintained model map (2,784+ models across all providers).
#[derive(Debug, Clone)]
pub struct Pricing {
    pub input_price: f64,
    pub output_price: f64,
    pub cache_write_price: f64,
    pub cache_read_price: f64,
}

// ── Core constructors ──────────────────────────────────────────────────────

impl Pricing {
    /// Load from explicit paths. Tries user override, then bundled, then defaults.
    pub fn load_with_overrides(
        user_override: Option<&Path>,
        bundled_path: &Path,
    ) -> Self {
        let path = user_override
            .filter(|p| p.exists())
            .or_else(|| if bundled_path.exists() { Some(bundled_path) } else { None });

        if let Some(p) = path {
            match std::fs::read_to_string(p) {
                Ok(content) => {
                    match serde_json::from_str::<HashMap<String, ModelPricing>>(&content) {
                        Ok(models) => {
                            let default = models
                                .get("gpt-4o-mini")
                                .or_else(|| models.values().next())
                                .cloned()
                                .unwrap_or_default();
                            log::info!(
                                "Loaded pricing for {} models from {}",
                                models.len(),
                                p.display()
                            );
                            let flat = Self::from_default(default);
                            log::info!("Loaded pricing for {} models from {}", models.len(), p.display());
                            return flat;
                        }
                        Err(e) => log::error!("Failed to parse LiteLLM pricing JSON: {}", e),
                    }
                }
                Err(e) => log::error!("Failed to read pricing file: {}", e),
            }
        }

        log::warn!("No pricing file found, using hardcoded defaults");
        Self::default()
    }

    /// Load from `config_dir/litellm.json`, falling back to bundled resource.
    ///
    /// This is the primary entry point used by `Config::load()` (config.rs).
    pub fn load(config_dir: &Path) -> Self {
        let override_path = config_dir.join("litellm.json");
        let user_override = if override_path.exists() {
            Some(override_path.as_path())
        } else {
            None
        };
        Self::load_with_overrides(user_override, &resolve_bundled_path())
    }

    fn from_default(default: ModelPricing) -> Self {
        Self {
            input_price: default.input_cost_per_token * 1_000_000.0,
            output_price: default.output_cost_per_token * 1_000_000.0,
            cache_write_price: default.cache_creation_input_token_cost * 1_000_000.0,
            cache_read_price: default.cache_read_input_token_cost * 1_000_000.0,
        }
    }
}

// ── Default (no LiteLLM file found) ───────────────────────────────────────

impl Default for Pricing {
    fn default() -> Self {
        Self::from_default(ModelPricing::default())
    }
}

// ── Bundled resource path resolution ──────────────────────────────────────

fn resolve_bundled_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            // Exe at: target/debug/loc-dock.exe
            // Resource at: resources/pricing/litellm.json
            // Path from exe: ../../resources/pricing/litellm.json

            // Try crawling up the directory tree from exe dir
            let mut probe = exe_dir.to_path_buf();
            for _ in 0..10 {
                // Max 10 levels up to prevent infinite loops
                let candidate = probe.join("resources").join("pricing").join("litellm.json");
                if candidate.exists() {
                    return candidate;
                }
                if !probe.pop() {
                    break;
                }
            }
        }
    }
    // Last-resort fallback for tests / unusual setups
    Path::new("resources/pricing/litellm.json").to_path_buf()
}
