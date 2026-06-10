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
}

const DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

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

  const update = (field: keyof Settings, value: string | number) => {
    const updated = { ...settings, [field]: value };
    setSettings(updated);
    setDirty(JSON.stringify(updated) !== original.current);
    setSaved(false);
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await invoke("save_settings", { settings });
      original.current = JSON.stringify(settings);
      setDirty(false);
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
