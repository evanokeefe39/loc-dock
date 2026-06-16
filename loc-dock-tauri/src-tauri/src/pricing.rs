use serde::{Deserialize, Serialize};
use std::path::Path;

/// Per-million-token pricing config, loaded from pricing.yaml in the config dir.
/// Users can edit this file without recompiling when Anthropic/DeepSeek/etc.
/// change their pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    pub input_price: f64,
    pub output_price: f64,
    pub cache_write_price: f64,
    pub cache_read_price: f64,
}

impl Default for Pricing {
    fn default() -> Self {
        Self {
            // Anthropic Claude 3/3.5 Sonnet per-million-token pricing (USD)
            // https://docs.anthropic.com/en/docs/about-claude/pricing
            input_price: 15.00,
            output_price: 75.00,
            cache_write_price: 18.75,
            cache_read_price: 1.50,
        }
    }
}

impl Pricing {
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join("pricing.yaml");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_yaml::from_str(&content) {
                    Ok(p) => {
                        log::info!("Loaded pricing from {}", path.display());
                        return p;
                    }
                    Err(e) => log::error!("Failed to parse pricing.yaml: {}, using defaults", e),
                },
                Err(e) => log::error!("Failed to read pricing.yaml: {}, using defaults", e),
            }
        }
        Self::create_default(&path);
        Self::default()
    }

    fn create_default(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = r##"# LOC Dock Pricing
# Per-million-token costs in USD. Update these when provider pricing changes.
# Currently set for Anthropic Claude 3/3.5 Sonnet.
# See https://docs.anthropic.com/en/docs/about-claude/pricing

input_price: 15.00
output_price: 75.00
cache_write_price: 18.75
cache_read_price: 1.50
"##;
        match std::fs::write(path, content) {
            Ok(_) => log::info!("Created default pricing at {}", path.display()),
            Err(e) => log::warn!("Failed to create default pricing: {}", e),
        }
    }
}