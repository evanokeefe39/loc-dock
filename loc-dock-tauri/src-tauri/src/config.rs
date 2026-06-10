use std::path::PathBuf;

pub struct Config {
    pub repos_dir: PathBuf,
    pub claude_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub timezone: String,
    pub day_start_hour: u32,
    pub week_start_day: u32,
    pub config_dir: PathBuf,
    pub theme_path: PathBuf,
    pub autostart: bool,
}

impl Config {
    pub fn load() -> Self {
        // Try loading .env from config dir first
        let config_dir = Self::config_dir();
        let env_path = config_dir.join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        let repos_dir = std::env::var("LOCDOCK_REPOS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("repos"));

        let claude_dir = std::env::var("LOCDOCK_CLAUDE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".claude"));

        let projects_dir = claude_dir.join("projects");

        let timezone = std::env::var("LOCDOCK_TIMEZONE")
            .unwrap_or_else(|_| "Europe/Berlin".to_string());

        let day_start_hour: u32 = std::env::var("LOCDOCK_DAY_START_HOUR")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7)
            .min(23);

        let week_start_day: u32 = std::env::var("LOCDOCK_WEEK_START_DAY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
            .min(6);

        let theme_path = std::env::var("LOCDOCK_THEME_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| config_dir.join("theme.yaml"));

        let autostart = std::env::var("LOCDOCK_AUTOSTART")
            .map(|s| s == "true" || s == "1")
            .unwrap_or(false);

        Config {
            repos_dir,
            claude_dir,
            projects_dir,
            timezone,
            day_start_hour,
            week_start_day,
            config_dir,
            theme_path,
            autostart,
        }
    }

    fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("loc-dock")
    }
}
