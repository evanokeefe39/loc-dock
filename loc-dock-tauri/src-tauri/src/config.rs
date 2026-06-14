use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default = "default_repos_dir")]
    pub repos_dir: PathBuf,
    #[serde(default = "default_claude_dir")]
    pub claude_dir: PathBuf,
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
}

pub struct Config {
    pub settings: Settings,
    pub config_dir: PathBuf,
    pub projects_dir: PathBuf,
}

impl Config {
    pub fn load() -> Self {
        let config_dir = Self::config_dir();
        let settings = Settings::load(&config_dir);
        let projects_dir = settings.claude_dir.join("projects");
        Config { settings, config_dir, projects_dir }
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
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(settings) => return settings,
                    Err(e) => log::error!("Failed to parse settings.json: {}", e),
                },
                Err(e) => log::error!("Failed to read settings.json: {}", e),
            }
        }
        // Migrate from .env if it exists
        let env_path = config_dir.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
            let settings = Self::from_env();
            if let Err(e) = settings.save(config_dir) {
                log::error!("Failed to migrate .env to settings.json: {}", e);
            }
            log::info!("Migrated settings from .env to settings.json");
            return settings;
        }
        Self::default()
    }

    fn from_env() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Settings {
            repos_dir: std::env::var("LOCDOCK_REPOS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join("repos")),
            claude_dir: std::env::var("LOCDOCK_CLAUDE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".claude")),
            timezone: std::env::var("LOCDOCK_TIMEZONE")
                .unwrap_or_else(|_| "Europe/Berlin".to_string()),
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
            summary_enabled: std::env::var("LOCDOCK_SUMMARY_ENABLED")
                .map(|s| s != "false" && s != "0")
                .unwrap_or(true),
            llm_api_key: std::env::var("LOCDOCK_LLM_API_KEY")
                .or_else(|_| std::env::var("DEEPSEEK_API_KEY"))
                .ok(),
            llm_api_endpoint: std::env::var("LOCDOCK_LLM_ENDPOINT")
                .unwrap_or_else(|_| "https://api.deepseek.com/v1".to_string()),
            llm_model: std::env::var("LOCDOCK_LLM_MODEL")
                .unwrap_or_else(|_| "deepseek-chat".to_string()),
            summary_debounce_secs: 300,
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
        Settings {
            repos_dir: home.join("repos"),
            claude_dir: home.join(".claude"),
            timezone: "Europe/Berlin".to_string(),
            day_start_hour: 7,
            week_start_day: 0,
            theme_path: Config::config_dir().join("theme.yaml"),
            autostart: false,
            refresh_interval: 60,
            session_idle_timeout: 300,
            summary_enabled: true,
            llm_api_key: None,
            llm_api_endpoint: "https://api.deepseek.com/v1".to_string(),
            llm_model: "deepseek-chat".to_string(),
            summary_debounce_secs: 300,
        }
    }
}

fn default_repos_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("repos")
}

fn default_claude_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude")
}

fn default_timezone() -> String {
    "Europe/Berlin".to_string()
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
