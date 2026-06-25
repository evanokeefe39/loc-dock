use crate::pricing::Pricing;
use crate::source_adapter::DataSourceConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "default_repos_dir")]
    pub repos_dir: PathBuf,
    #[serde(default = "default_claude_dir")]
    pub claude_dir: PathBuf,
    #[serde(default = "default_pi_dir")]
    pub pi_dir: PathBuf,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "default_day_start_hour")]
    pub day_start_hour: u32,
    #[serde(default)]
    pub week_start_day: u32,
    #[serde(default = "default_theme_path")]
    pub theme_path: PathBuf,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u64,
    #[serde(default = "default_session_idle_timeout")]
    pub session_idle_timeout: u64,
    #[serde(default = "default_summary_enabled")]
    pub summary_enabled: bool,
    #[serde(default)]
    pub llm_api_key: Option<String>,
    #[serde(default = "default_llm_api_endpoint")]
    pub llm_api_endpoint: String,
    #[serde(default = "default_llm_model")]
    pub llm_model: String,
    #[serde(default = "default_summary_debounce_secs")]
    pub summary_debounce_secs: u64,
    #[serde(default = "default_summary_exclude_pattern")]
    pub summary_exclude_pattern: String,
    #[serde(default = "default_git_history_days")]
    pub git_history_days: u64,
    #[serde(default)]
    pub hide_repos_without_prs: bool,
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
    #[serde(default = "default_usage_cache_dir")]
    pub usage_cache_dir: PathBuf,
    /// User-configured data sources (replaces single claude_dir/pi_dir).
    #[serde(default)]
    pub data_sources: Vec<DataSourceConfig>,
    /// Optional path to a user-edited LiteLLM pricing JSON override.
    #[serde(default)]
    pub model_pricing_path: Option<PathBuf>,
}

pub struct Config {
    pub settings: Settings,
    pub config_dir: PathBuf,
    pub pricing: Pricing,
}

impl Config {
    pub fn load() -> Self {
        let config_dir = Self::config_dir();
        let settings = Settings::load(&config_dir);
        let pricing = Pricing::load(&config_dir);
        Config { settings, config_dir, pricing }
    }

    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("loc-dock")
    }

}

impl Settings {
    pub fn load(config_dir: &PathBuf) -> Self {
        let path = config_dir.join("settings.json");
        let mut migrated = false;
        let settings: Self = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    // v5→v6 migration: if claude_dir/pi_dir exist but data_sources is empty,
                    // create DataSourceConfig entries from the old fields.
                    #[derive(Deserialize)]
                    struct OldSettings {
                        claude_dir: Option<PathBuf>,
                        pi_dir: Option<PathBuf>,
                        #[serde(default)]
                        data_sources: Vec<DataSourceConfig>,
                    }
                    match serde_json::from_str::<OldSettings>(&content) {
                        Ok(old) if old.data_sources.is_empty() => {
                            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                            let cd = old.claude_dir.unwrap_or_else(|| home.join(".claude"));
                            let pd = old.pi_dir.unwrap_or_else(|| home.join(".pi"));
                            let mut s: Self = serde_json::from_str(&content)
                                .unwrap_or_else(|_| Self::default());
                            s.data_sources = vec![
                                DataSourceConfig {
                                    id: "claude-main".to_string(),
                                    adapter: "claude".to_string(),
                                    display_name: "Claude Code".to_string(),
                                    path: cd.join("projects"),
                                },
                                DataSourceConfig {
                                    id: "pi-main".to_string(),
                                    adapter: "pi".to_string(),
                                    display_name: "Pi".to_string(),
                                    path: pd.join("agent").join("sessions"),
                                },
                            ];
                            migrated = true;
                            s
                        }
                        Ok(_) => {
                            // Has data_sources already — deserialize normally
                            serde_json::from_str(&content).unwrap_or_else(|_| Self::default())
                        }
                        Err(_) => {
                            // Not v5 format either — try as current
                            match serde_json::from_str::<Self>(&content) {
                                Ok(s) => s,
                                Err(e) => {
                                    log::error!("Failed to parse settings.json: {}", e);
                                    Self::default()
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log::error!("Failed to read settings.json: {}", e);
                    Self::default()
                }
            }
        } else {
            // Migrate from .env if it exists
            let env_path = config_dir.join(".env");
            if env_path.exists() {
                let _ = dotenvy::from_path(&env_path);
                let s = Self::from_env();
                s
            } else {
                Self::default()
            }
        };

        if migrated {
            if let Err(e) = settings.save(config_dir) {
                log::error!("Failed to save migrated settings.json: {}", e);
            }
            log::info!("Migrated settings: claude_dir/pi_dir → data_sources");
        }

        settings
    }

    fn from_env() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let cd: PathBuf = std::env::var("LOCDOCK_CLAUDE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".claude"));
        let pd: PathBuf = std::env::var("LOCDOCK_PI_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".pi"));
        Settings {
            repos_dir: std::env::var("LOCDOCK_REPOS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("repos")),
            claude_dir: cd.clone(),
            pi_dir: pd.clone(),
            data_sources: vec![
                DataSourceConfig {
                    id: "claude-main".to_string(),
                    adapter: "claude".to_string(),
                    display_name: "Claude Code".to_string(),
                    path: cd.join("projects"),
                },
                DataSourceConfig {
                    id: "pi-main".to_string(),
                    adapter: "pi".to_string(),
                    display_name: "Pi".to_string(),
                    path: pd.join("agent").join("sessions"),
                },
            ],
            timezone: std::env::var("LOCDOCK_TIMEZONE")
                .unwrap_or_else(|_| default_timezone()),
            day_start_hour: std::env::var("LOCDOCK_DAY_START_HOUR")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7)
                .min(23),
            week_start_day: std::env::var("LOCDOCK_WEEK_START_DAY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
                .min(6),
            theme_path: std::env::var("LOCDOCK_THEME_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| Config::config_dir().join("theme.yaml")),
            autostart: std::env::var("LOCDOCK_AUTOSTART")
                .map(|s| s == "true" || s == "1")
                .unwrap_or(false),
            refresh_interval: 60,
            session_idle_timeout: 300,
            git_history_days: std::env::var("LOCDOCK_GIT_HISTORY_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(200)
                .min(3650),
            summary_enabled: std::env::var("LOCDOCK_SUMMARY_ENABLED")
                .map(|s| s != "false" && s != "0")
                .unwrap_or(true),
            llm_api_key: None,
            llm_api_endpoint: std::env::var("LOCDOCK_LLM_ENDPOINT")
                .unwrap_or_else(|_| "https://api.deepseek.com/v1".to_string()),
            llm_model: std::env::var("LOCDOCK_LLM_MODEL")
                .unwrap_or_else(|_| "deepseek-chat".to_string()),
            summary_debounce_secs: 300,
            summary_exclude_pattern: default_summary_exclude_pattern(),
            hide_repos_without_prs: false,
            log_dir: default_log_dir(),
            usage_cache_dir: default_usage_cache_dir(),
            model_pricing_path: None,
        }
    }

    pub fn save(&self, config_dir: &PathBuf) -> Result<(), String> {
        std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
        let path = config_dir.join("settings.json");
        let content = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, content).map_err(|e| e.to_string())
    }
}

impl Default for Settings {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let cd = home.join(".claude");
        let pd = home.join(".pi");
        Settings {
            repos_dir: home.join("repos"),
            claude_dir: cd.clone(),
            pi_dir: pd.clone(),
            data_sources: vec![
                DataSourceConfig {
                    id: "claude-main".to_string(),
                    adapter: "claude".to_string(),
                    display_name: "Claude Code".to_string(),
                    path: cd.join("projects"),
                },
                DataSourceConfig {
                    id: "pi-main".to_string(),
                    adapter: "pi".to_string(),
                    display_name: "Pi".to_string(),
                    path: pd.join("agent").join("sessions"),
                },
            ],
            timezone: default_timezone(),
            day_start_hour: 7,
            week_start_day: 0,
            theme_path: Config::config_dir().join("theme.yaml"),
            autostart: false,
            refresh_interval: 60,
            session_idle_timeout: 300,
            git_history_days: 200,
            summary_enabled: true,
            llm_api_key: None,
            llm_api_endpoint: "https://api.deepseek.com/v1".to_string(),
            llm_model: "deepseek-chat".to_string(),
            summary_debounce_secs: 300,
            summary_exclude_pattern: default_summary_exclude_pattern(),
            hide_repos_without_prs: false,
            log_dir: default_log_dir(),
            usage_cache_dir: default_usage_cache_dir(),
            model_pricing_path: None,
        }
    }
}

fn default_repos_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("repos")
}

fn default_claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude")
}

fn default_pi_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".pi")
}

fn default_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string())
}

fn default_day_start_hour() -> u32 {
    7
}

fn default_theme_path() -> PathBuf {
    Config::config_dir().join("theme.yaml")
}

fn default_refresh_interval() -> u64 {
    60
}

fn default_session_idle_timeout() -> u64 {
    300
}

fn default_git_history_days() -> u64 { 200 }

fn default_summary_enabled() -> bool {
    true
}

fn default_llm_api_endpoint() -> String {
    "https://api.deepseek.com/v1".to_string()
}

fn default_llm_model() -> String {
    "deepseek-chat".to_string()
}

fn default_summary_debounce_secs() -> u64 {
    300
}

fn default_summary_exclude_pattern() -> String {
    "^(chore|docs|style|ci):".to_string()
}

fn default_log_dir() -> PathBuf {
    Config::config_dir()
}

fn default_usage_cache_dir() -> PathBuf {
    Config::config_dir()
}
