import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Settings {
  repos_dir: string;
  claude_dir: string;
  timezone: string;
  day_start_hour: number;
  week_start_day: number;
  theme_path: string;
}

const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

interface Props {
  visible: boolean;
  onClose: () => void;
}

export function SettingsPanel({ visible, onClose }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (visible) {
      invoke<Settings>("get_settings").then(setSettings).catch(console.error);
      setSaved(false);
    }
  }, [visible]);

  if (!visible || !settings) return null;

  const update = (field: keyof Settings, value: string | number) => {
    setSettings({ ...settings, [field]: value });
    setSaved(false);
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await invoke("save_settings", { settings });
      setSaved(true);
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
          <button className="settings-close" onClick={onClose}>Done</button>
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
          <span>Timezone</span>
          <input value={settings.timezone} onChange={e => update("timezone", e.target.value)} />
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

        <div className="settings-divider" />

        <label>
          <span>Theme file</span>
          <input value={settings.theme_path} onChange={e => update("theme_path", e.target.value)} />

        <div className="settings-footer">
          <button className="settings-save" onClick={handleSave} disabled={saving}>
            {saving ? "Saving..." : saved ? "Saved ✓" : "Save"}
          </button>
          <span className="settings-hint">Restart to apply changes</span>
        </div>
      </div>
    </div>
  );
}
