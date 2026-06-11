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
  const [saved, setSaved] = useState(false);
  const original = useRef<string>("");

  useEffect(() => {
    if (visible) {
      invoke<Settings>("get_settings").then((s) => {
        setSettings(s);
        original.current = JSON.stringify(s);
        setDirty(false);
        setSaved(false);
      }).catch(console.error);
    }
  }, [visible]);

  if (!visible || !settings) return null;

  const update = (field: keyof Settings, value: string | number | boolean) => {
    const updated = { ...settings, [field]: value };
    setSettings(updated);
    setDirty(JSON.stringify(updated) !== original.current);
    setSaved(false);
  };

  const VISUAL_ONLY: (keyof Settings)[] = ["theme_path", "autostart"];

  const needsRestart = () => {
    if (!original.current) return false;
    const prev = JSON.parse(original.current) as Settings;
    return Object.keys(settings!).some(
      k => !VISUAL_ONLY.includes(k as keyof Settings) && JSON.stringify((settings as Record<string, unknown>)[k]) !== JSON.stringify((prev as Record<string, unknown>)[k])
    );
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await invoke("save_settings", { settings });
      const restart = needsRestart();
      original.current = JSON.stringify(settings);
      setDirty(false);
      setSaved(restart);
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
            onChange={e => update("refresh_interval", Math.max(10, parseInt(e.target.value) || 60))}
          />
        </label>

        <label title="Sessions with no activity within this period are considered inactive">
          <span>Session idle timeout (seconds)</span>
          <input
            type="number"
            min={30}
            max={3600}
            value={settings.session_idle_timeout}
            onChange={e => update("session_idle_timeout", Math.max(30, parseInt(e.target.value) || 300))}
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

        {saved && (
          <button className="settings-restart" onClick={() => {
            invoke("restart_app").catch(() => getCurrentWindow().close());
          }}>
            Restart to apply changes
          </button>
        )}
      </div>
    </div>
  );
}
