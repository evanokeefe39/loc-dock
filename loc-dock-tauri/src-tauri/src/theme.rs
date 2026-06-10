use serde::{Deserialize, Serialize};
use std::path::Path;
use log::warn;

fn default_alpha() -> f64 { 0.92 }
fn default_bg() -> String { "#202020".into() }
fn default_chart_bg() -> String { "#181818".into() }
fn default_tooltip_bg() -> String { "#2a2a2a".into() }
fn default_text() -> String { "#e0e0e0".into() }
fn default_text_dim() -> String { "#6b7280".into() }
fn default_axis() -> String { "#333333".into() }
fn default_loc_add() -> String { "#34d399".into() }
fn default_loc_del() -> String { "#ef4444".into() }
fn default_cost() -> String { "#a78bfa".into() }
fn default_sessions() -> String { "#f97316".into() }
fn default_tok_input() -> String { "#e0e0e0".into() }
fn default_tok_output() -> String { "#f472b6".into() }
fn default_tok_cache_write() -> String { "#facc15".into() }
fn default_tok_cache_read() -> String { "#38bdf8".into() }

#[derive(Serialize, Deserialize, Clone)]
pub struct Theme {
    #[serde(default = "default_alpha")]
    pub alpha: f64,
    #[serde(default = "default_bg")]
    pub bg: String,
    #[serde(default = "default_chart_bg")]
    pub chart_bg: String,
    #[serde(default = "default_tooltip_bg")]
    pub tooltip_bg: String,
    #[serde(default = "default_text")]
    pub text: String,
    #[serde(default = "default_text_dim")]
    pub text_dim: String,
    #[serde(default = "default_axis")]
    pub axis: String,
    #[serde(default = "default_loc_add")]
    pub loc_add: String,
    #[serde(default = "default_loc_del")]
    pub loc_del: String,
    #[serde(default = "default_cost")]
    pub cost: String,
    #[serde(default = "default_sessions")]
    pub sessions: String,
    #[serde(default = "default_tok_input")]
    pub tok_input: String,
    #[serde(default = "default_tok_output")]
    pub tok_output: String,
    #[serde(default = "default_tok_cache_write")]
    pub tok_cache_write: String,
    #[serde(default = "default_tok_cache_read")]
    pub tok_cache_read: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            alpha: default_alpha(),
            bg: default_bg(),
            chart_bg: default_chart_bg(),
            tooltip_bg: default_tooltip_bg(),
            text: default_text(),
            text_dim: default_text_dim(),
            axis: default_axis(),
            loc_add: default_loc_add(),
            loc_del: default_loc_del(),
            cost: default_cost(),
            sessions: default_sessions(),
            tok_input: default_tok_input(),
            tok_output: default_tok_output(),
            tok_cache_write: default_tok_cache_write(),
            tok_cache_read: default_tok_cache_read(),
        }
    }
}

impl Theme {
    pub fn load(theme_path: &Path) -> Self {
        if !theme_path.exists() {
            Self::create_default(theme_path);
        }
        let path = theme_path;
        let mut theme = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    match serde_yaml::from_str::<Theme>(&content) {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("Bad theme.yaml, using defaults: {}", e);
                            Theme::default()
                        }
                    }
                }
                Err(e) => {
                    warn!("Cannot read theme.yaml: {}", e);
                    Theme::default()
                }
            }
        } else {
            Theme::default()
        };
        theme.validate();
        theme
    }

    fn create_default(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content = r##"# LOC Dock Theme
# Edit colors below. All values are hex (#RRGGBB).

alpha: 0.92              # 0.0 = invisible, 1.0 = opaque
bg: "#202020"            # main background
chart_bg: "#181818"      # chart background
tooltip_bg: "#2a2a2a"    # cost breakdown tooltip

text: "#e0e0e0"          # primary text
text_dim: "#6b7280"      # secondary text, labels
axis: "#333333"          # chart axis lines

loc_add: "#34d399"       # lines added
loc_del: "#ef4444"       # lines deleted
cost: "#a78bfa"          # cost label and chart
sessions: "#f97316"      # active session count

tok_input: "#e0e0e0"     # input tokens
tok_output: "#f472b6"    # output tokens
tok_cache_write: "#facc15"  # cache write tokens
tok_cache_read: "#38bdf8"   # cache read tokens
"##;
        match std::fs::write(path, content) {
            Ok(_) => log::info!("Created default theme at {}", path.display()),
            Err(e) => warn!("Failed to create default theme: {}", e),
        }
    }

    fn validate(&mut self) {
        self.alpha = self.alpha.clamp(0.0, 1.0);
        let defaults = Theme::default();
        let hex_re = regex::Regex::new(r"^#[0-9a-fA-F]{6}$").unwrap();
        // Validate each color field, reset to default if invalid
        macro_rules! check_color {
            ($field:ident) => {
                if !hex_re.is_match(&self.$field) {
                    warn!("theme: invalid color for {}, using default", stringify!($field));
                    self.$field = defaults.$field.clone();
                }
            };
        }
        check_color!(bg);
        check_color!(chart_bg);
        check_color!(tooltip_bg);
        check_color!(text);
        check_color!(text_dim);
        check_color!(axis);
        check_color!(loc_add);
        check_color!(loc_del);
        check_color!(cost);
        check_color!(sessions);
        check_color!(tok_input);
        check_color!(tok_output);
        check_color!(tok_cache_write);
        check_color!(tok_cache_read);
    }
}
