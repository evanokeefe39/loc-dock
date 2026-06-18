import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Save } from "lucide-react";

interface DataSourceConfig {
  id: string;
  adapter: string;
  display_name: string;
  path: string;
  enabled: boolean;
}

interface Settings {
  repos_dir: string;
  data_sources: DataSourceConfig[];
  model_pricing_path: string | null;
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
  hide_repos_without_prs: boolean;
  log_dir: string;
  usage_cache_dir: string;
}

const DATA_ROWS: { key: keyof Settings; label: string; cmd: string; what: string }[] = [
  { key: "usage_cache_dir",   label: "Usage cache",   cmd: "reset_usage_cache",   what: "usage cache" },
  { key: "log_dir",           label: "Job logs",       cmd: "clear_job_logs",      what: "job logs" },
];

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
  const [confirmReset, setConfirmReset] = useState<string | null>(null);
  const [showAddSource, setShowAddSource] = useState(false);
  const [addingSource, setAddingSource] = useState(false);
  const [newSource, setNewSource] = useState<DataSourceConfig>({
    id: '',
    adapter: 'pi',
    display_name: '',
    path: '',
    enabled: true,
  });
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

  const VISUAL_ONLY: (keyof Settings)[] = ["theme_path", "autostart", "hide_repos_without_prs"];

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

  const activeRow = confirmReset ? DATA_ROWS.find(r => r.cmd === confirmReset) : null;

  const handleAddSource = async () => {
    setAddingSource(true);
    try {
      const id = newSource.adapter + '-' + Date.now().toString(36);
      await invoke('add_source', { source: { ...newSource, id } });
      const s = await invoke<Settings>('get_settings');
      setSettings(s);
      original.current = JSON.stringify(s);
      setDirty(false);
      setShowAddSource(false);
      setNewSource({ id: '', adapter: 'pi', display_name: '', path: '', enabled: true });
    } catch (e) {
      console.error('Failed to add source:', e);
      alert('Failed to add source: ' + e);
    }
    setAddingSource(false);
  };

  const handleRemoveSource = async (id: string) => {
    if (!confirm('Remove this data source? Entries already imported will be kept.')) return;
    try {
      await invoke('remove_source', { id });
      const s = await invoke<Settings>('get_settings');
      setSettings(s);
      original.current = JSON.stringify(s);
      setDirty(false);
    } catch (e) {
      console.error('Failed to remove source:', e);
    }
  };

  const handleToggleSource = async (id: string) => {
    try {
      await invoke('toggle_source', { id });
      const s = await invoke<Settings>('get_settings');
      setSettings(s);
      original.current = JSON.stringify(s);
      setDirty(false);
    } catch (e) {
      console.error('Failed to toggle source:', e);
    }
  };

  return (
    <div className="settings-overlay">
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
      <div className="settings-scroll">
        <div className="settings-panel">

        {/* ── General ── */}
        <div className="settings-section-heading">General</div>

        <label>
          <span>Repos directory</span>
          <input value={settings.repos_dir} onChange={e => update("repos_dir", e.target.value)} />
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

        {/* ── Data ── */}
        <div className="settings-section-heading">Data</div>

        <div className="settings-data-section-note">
          <span className="settings-data-section-label">Session Sources</span>
          <button
            className="settings-add-source-btn"
            onClick={() => setShowAddSource(true)}
            disabled={showAddSource}
          >
            + Add Source
          </button>
        </div>

        {showAddSource && (
          <div className="settings-add-source-form">
            <label>
              <span>Type</span>
              <select value={newSource.adapter} onChange={e => setNewSource({...newSource, adapter: e.target.value})}>
                <option value="pi">Pi</option>
                <option value="claude">Claude Code</option>
                <option value="codex">Codex CLI</option>
              </select>
            </label>
            <label>
              <span>Display name</span>
              <input
                value={newSource.display_name}
                onChange={e => setNewSource({...newSource, display_name: e.target.value})}
                placeholder="My Pi sessions"
              />
            </label>
            <label>
              <span>Session directory</span>
              <input
                value={newSource.path}
                onChange={e => setNewSource({...newSource, path: e.target.value})}
                placeholder="~/.pi/agent/sessions"
              />
            </label>
            <div className="settings-add-source-actions">
              <button
                className="settings-action-btn settings-action-primary"
                onClick={handleAddSource}
                disabled={addingSource}
              >
                {addingSource ? 'Adding...' : 'Add'}
              </button>
              <button className="settings-action-btn" onClick={() => setShowAddSource(false)}>
                Cancel
              </button>
            </div>
          </div>
        )}

        {settings.data_sources && settings.data_sources.map(source => (
          <div key={source.id} className="settings-source-row">
            <div className="settings-source-info">
              <span className="settings-source-adapter">{source.adapter}</span>
              <span className="settings-source-name">{source.display_name}</span>
              <span className="settings-source-path">{source.path}</span>
            </div>
            <div className="settings-source-actions">
              <label className="settings-toggle">
                <input
                  type="checkbox"
                  checked={source.enabled}
                  onChange={() => handleToggleSource(source.id)}
                />
                Enabled
              </label>
              <button
                className="settings-delete-source-btn"
                onClick={() => handleRemoveSource(source.id)}
                title="Remove this source"
              >
                Delete
              </button>
            </div>
          </div>
        ))}

        {DATA_ROWS.map(row => (
          <div key={row.key} className="settings-data-row">
            <label>
              <span>{row.label}</span>
              <input
                value={settings[row.key] as string}
                onChange={e => update(row.key, e.target.value)}
              />
            </label>
            <button
              className="settings-inline-reset"
              onClick={() => setConfirmReset(row.cmd)}
              title={`Clear ${row.what}`}
            >
              clear
            </button>
          </div>
        ))}

        {activeRow && (
          <div className="settings-confirm-modal">
            <span>Delete {activeRow.what} and restart?</span>
            <div className="settings-confirm-actions">
              <button
                className="settings-action-btn settings-action-danger"
                onClick={async () => {
                  try {
                    await invoke(activeRow.cmd);
                    setConfirmReset(null);
                    invoke("restart_app").catch(() => getCurrentWindow().close());
                  } catch (e) {
                    console.error("Reset failed:", e);
                  }
                }}
              >
                Yes, reset
              </button>
              <button className="settings-action-btn" onClick={() => setConfirmReset(null)}>
                Cancel
              </button>
            </div>
          </div>
        )}

        <hr className="settings-data-divider" />

        <label title="Path to a LiteLLM-format pricing JSON to override bundled prices. Leave empty for bundled defaults.">
          <span>Pricing override (optional)</span>
          <input
            value={settings.model_pricing_path ?? ''}
            onChange={e => update('model_pricing_path', e.target.value || null)}
            placeholder="~/.config/loc-dock/litellm.json"
          />
        </label>

        {/* ── AI Summaries ── */}
        <div className="settings-section-heading">AI Summaries</div>

        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.summary_enabled}
            onChange={e => update("summary_enabled", e.target.checked)}
          />
          <span>Enable AI commit summaries</span>
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

        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.hide_repos_without_prs}
            onChange={e => update("hide_repos_without_prs", e.target.checked)}
          />
          <span>Hide repos without PRs</span>
        </label>

        </div>
      </div>
    </div>
  );
}
