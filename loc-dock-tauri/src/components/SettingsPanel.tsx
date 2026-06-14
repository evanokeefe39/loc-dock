import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Save } from "lucide-react";

interface Settings {
  repos_dir: string;
  claude_dir: string;
  timezone: string;
  day_start_hour: number;
  week_start_day: number;
  theme_path: string;
  autostart: boolean;
  refresh_interval: number;
  session_idle_timeout: number;
  summary_enabled: boolean;
  llm_api_key: string | null;
  llm_api_endpoint: string;
  llm_model: string;
  summary_debounce_secs: number;
  summary_exclude_pattern: string;
}

const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

const TIMEZONES: [string, string][] = [
  ["Pacific/Midway", "(GMT -11:00) Midway Island, Samoa"],
  ["Pacific/Honolulu", "(GMT -10:00) Hawaii"],
  ["America/Anchorage", "(GMT -9:00) Alaska"],
  ["America/Los_Angeles", "(GMT -8:00) Pacific Time (US & Canada)"],
  ["America/Denver", "(GMT -7:00) Mountain Time (US & Canada)"],
  ["America/Chicago", "(GMT -6:00) Central Time (US & Canada)"],
  ["America/Mexico_City", "(GMT -6:00) Mexico City"],
  ["America/New_York", "(GMT -5:00) Eastern Time (US & Canada)"],
  ["America/Bogota", "(GMT -5:00) Bogota, Lima"],
  ["America/Caracas", "(GMT -4:30) Caracas"],
  ["America/Halifax", "(GMT -4:00) Atlantic Time (Canada)"],
  ["America/St_Johns", "(GMT -3:30) Newfoundland"],
  ["America/Sao_Paulo", "(GMT -3:00) Brazil, Buenos Aires"],
  ["Atlantic/Azores", "(GMT -1:00) Azores"],
  ["UTC", "(GMT) UTC"],
  ["Europe/London", "(GMT +0:00) London, Lisbon, Casablanca"],
  ["Europe/Berlin", "(GMT +1:00) Brussels, Berlin, Madrid, Paris"],
  ["Europe/Helsinki", "(GMT +2:00) Helsinki, Kyiv, Bucharest, Athens"],
  ["Africa/Johannesburg", "(GMT +2:00) South Africa"],
  ["Europe/Istanbul", "(GMT +3:00) Istanbul"],
  ["Europe/Moscow", "(GMT +3:00) Moscow, St. Petersburg"],
  ["Asia/Tehran", "(GMT +3:30) Tehran"],
  ["Asia/Dubai", "(GMT +4:00) Abu Dhabi, Dubai, Tbilisi"],
  ["Asia/Kabul", "(GMT +4:30) Kabul"],
  ["Asia/Karachi", "(GMT +5:00) Islamabad, Karachi"],
  ["Asia/Kolkata", "(GMT +5:30) Mumbai, New Delhi"],
  ["Asia/Kathmandu", "(GMT +5:45) Kathmandu"],
  ["Asia/Dhaka", "(GMT +6:00) Dhaka, Almaty"],
  ["Asia/Bangkok", "(GMT +7:00) Bangkok, Hanoi, Jakarta"],
  ["Asia/Shanghai", "(GMT +8:00) Beijing, Singapore, Hong Kong"],
  ["Australia/Perth", "(GMT +8:00) Perth"],
  ["Asia/Tokyo", "(GMT +9:00) Tokyo, Seoul, Osaka"],
  ["Australia/Adelaide", "(GMT +9:30) Adelaide, Darwin"],
  ["Australia/Sydney", "(GMT +10:00) Sydney, Melbourne, Guam"],
  ["Pacific/Auckland", "(GMT +12:00) Auckland, Wellington, Fiji"],
];

interface Props {
  visible: boolean;
  onClose: () => void;
}

export function SettingsPanel({ visible, onClose }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);
  const original = useRef<string>("");

  useEffect(() => {
    if (visible) {
      invoke<Settings>("get_settings").then((s) => {
        setSettings(s);
        original.current = JSON.stringify(s);
        setDirty(false);
              }).catch(console.error);
    }
  }, [visible]);

  if (!visible || !settings) return null;

  const update = (field: keyof Settings, value: string | number | boolean | null) => {
    const updated = { ...settings, [field]: value };
    setSettings(updated);
    setDirty(JSON.stringify(updated) !== original.current);
      };

  const VISUAL_ONLY: (keyof Settings)[] = ["theme_path", "autostart"];

  const needsRestart = () => {
    if (!original.current) return false;
    const prev = JSON.parse(original.current) as Settings;
    return Object.keys(settings!).some(
      k => !VISUAL_ONLY.includes(k as keyof Settings) && JSON.stringify(settings![k as keyof Settings]) !== JSON.stringify(prev[k as keyof Settings])
    );
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      const restart = needsRestart();
      await invoke("save_settings", { settings });
      original.current = JSON.stringify(settings);
      setDirty(false);
      if (restart) {
        invoke("restart_app").catch(() => getCurrentWindow().close());
      }
    } catch (e) {
      console.error("Save failed:", e);
    }
    setSaving(false);
  };

  return (
    <div className="settings-overlay">
      <div className="settings-panel">
        <div className="settings-header">
          <span>Settings</span>
          <div className="settings-header-actions">
            <button
              className="settings-save-btn"
              onClick={handleSave}
              disabled={!dirty || saving}
              title="Save settings"
            >
              <Save size={12} />
            </button>
            <button className="settings-close" onClick={onClose}>Back</button>
          </div>
        </div>

        <label>
          <span>Repos directory</span>
          <input value={settings.repos_dir} onChange={e => update("repos_dir", e.target.value)} />
        </label>

        <label>
          <span>Claude directory</span>
          <input value={settings.claude_dir} onChange={e => update("claude_dir", e.target.value)} />
        </label>

        <label>
          <span>Theme file</span>
          <input value={settings.theme_path} onChange={e => update("theme_path", e.target.value)} />
        </label>

        <label>
          <span>Timezone</span>
          <select value={settings.timezone} onChange={e => update("timezone", e.target.value)}>
            {!TIMEZONES.some(([val]) => val === settings.timezone) && (
              <option value={settings.timezone}>{settings.timezone}</option>
            )}
            {TIMEZONES.map(([val, label]) => (
              <option key={val} value={val}>{label}</option>
            ))}
          </select>
        </label>

        <label>
          <span>Day starts at</span>
          <select value={settings.day_start_hour} onChange={e => update("day_start_hour", parseInt(e.target.value))}>
            {Array.from({ length: 24 }, (_, i) => (
              <option key={i} value={i}>{i.toString().padStart(2, "0")}:00</option>
            ))}
          </select>
        </label>

        <label>
          <span>Week starts on</span>
          <select value={settings.week_start_day} onChange={e => update("week_start_day", parseInt(e.target.value))}>
            {DAYS.map((d, i) => (
              <option key={i} value={i}>{d}</option>
            ))}
          </select>
        </label>

        <label>
          <span>Refresh interval (seconds)</span>
          <input
            type="number"
            min={10}
            max={600}
            value={settings.refresh_interval}
            onChange={e => update("refresh_interval", parseInt(e.target.value) || 0)}
          />
        </label>

        <label title="Sessions with no activity within this period are considered inactive">
          <span>Session idle timeout (seconds)</span>
          <input
            type="number"
            min={30}
            max={3600}
            value={settings.session_idle_timeout}
            onChange={e => update("session_idle_timeout", parseInt(e.target.value) || 0)}
          />
        </label>

        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.autostart}
            onChange={e => update("autostart", e.target.checked)}
          />
          <span>Start on login</span>
        </label>

        <div className="settings-divider" />

        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.summary_enabled}
            onChange={e => update("summary_enabled", e.target.checked)}
          />
          <span>AI commit summaries</span>
        </label>

        <label title="API key for an OpenAI-compatible LLM provider (e.g. DeepSeek, OpenAI, OpenRouter)">
          <span>LLM API key</span>
          <input
            type="password"
            value={settings.llm_api_key ?? ""}
            onChange={e => update("llm_api_key", e.target.value || null)}
            placeholder="sk-..."
          />
        </label>

        <label title="Base URL for the chat completions API (OpenAI-compatible)">
          <span>LLM endpoint</span>
          <input
            value={settings.llm_api_endpoint}
            onChange={e => update("llm_api_endpoint", e.target.value)}
            placeholder="https://api.deepseek.com/v1"
          />
        </label>

        <label title="Model name to use for summaries">
          <span>LLM model</span>
          <input
            value={settings.llm_model}
            onChange={e => update("llm_model", e.target.value)}
            placeholder="deepseek-chat"
          />
        </label>

        <label title="Minimum seconds between LLM calls (prevents excessive API usage)">
          <span>Summary debounce (seconds)</span>
          <input
            type="number"
            min={60}
            max={3600}
            value={settings.summary_debounce_secs}
            onChange={e => update("summary_debounce_secs", parseInt(e.target.value) || 0)}
          />
        </label>

        <label title="Regex to exclude commit messages from summaries (e.g. ^(chore|docs|style|ci): for conventional commits). Leave empty to include all.">
          <span>Exclude pattern (regex)</span>
          <input
            value={settings.summary_exclude_pattern}
            onChange={e => update("summary_exclude_pattern", e.target.value)}
            placeholder="^(chore|docs|style|ci):"
          />
        </label>

      </div>
    </div>
  );
}
