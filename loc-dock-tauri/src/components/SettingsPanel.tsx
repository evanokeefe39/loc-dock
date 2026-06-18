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
  const [editingSource, setEditingSource] = useState<DataSourceConfig | null>(null);
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

  const handleSaveSource = async () => {
    if (!editingSource) return;
    try {
      await invoke('update_source', { id: editingSource.id, source: editingSource });
      const s = await invoke<Settings>('get_settings');
      setSettings(s);
      original.current = JSON.stringify(s);
      setDirty(false);
      setEditingSource(null);
    } catch (e) {
      console.error('Failed to update source:', e);
      alert('Failed to update source: ' + e);
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

        <div className="settings-sources-gallery">

          {settings.data_sources && settings.data_sources.map(source => (
            <div key={source.id} className="settings-source-card" onClick={() => setEditingSource(source)}>
              <div className={`settings-source-icon ${source.adapter}`}>
                {source.adapter === 'pi' ? (
                  <svg viewBox="0 0 800 800" width="22" height="22" fill="#fff">
                    <path fill-rule="evenodd" d="M165.29 165.29H517.36V400H400V517.36H282.65V634.72H165.29ZM282.65 282.65V400H400V282.65Z"/>
                    <path d="M517.36 400H634.72V634.72H517.36Z"/>
                  </svg>
                ) : source.adapter === 'claude' ? (
                  <svg viewBox="0 0 24 24" width="22" height="22" fill="#fff">
                    <path d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z"/>
                  </svg>
                ) : (
                  <svg viewBox="0 0 24 24" width="22" height="22" fill="#fff">
                    <path d="M22.282 9.821a6 6 0 0 0-.516-4.91 6.05 6.05 0 0 0-6.51-2.9A6.065 6.065 0 0 0 4.981 4.18a6 6 0 0 0-3.998 2.9 6.05 6.05 0 0 0 .743 7.097 5.98 5.98 0 0 0 .51 4.911 6.05 6.05 0 0 0 6.515 2.9A6 6 0 0 0 13.26 24a6.06 6.06 0 0 0 5.772-4.206 6 6 0 0 0 3.997-2.9 6.06 6.06 0 0 0-.747-7.073M13.26 22.43a4.48 4.48 0 0 1-2.876-1.04l.141-.081 4.779-2.758a.8.8 0 0 0 .392-.681v-6.737l2.02 1.168a.07.07 0 0 1 .038.052v5.583a4.504 4.504 0 0 1-4.494 4.494M3.6 18.304a4.47 4.47 0 0 1-.535-3.014l.142.085 4.783 2.759a.77.77 0 0 0 .78 0l5.843-3.369v2.332a.08.08 0 0 1-.033.062L9.74 19.95a4.5 4.5 0 0 1-6.14-1.646M2.34 7.896a4.5 4.5 0 0 1 2.366-1.973V11.6a.77.77 0 0 0 .388.677l5.815 3.354-2.02 1.168a.08.08 0 0 1-.071 0l-4.83-2.786A4.504 4.504 0 0 1 2.34 7.872zm16.597 3.855-5.833-3.387L15.119 7.2a.08.08 0 0 1 .071 0l4.83 2.791a4.494 4.494 0 0 1-.676 8.105v-5.678a.79.79 0 0 0-.407-.667m2.01-3.023-.141-.085-4.774-2.782a.78.78 0 0 0-.785 0L9.409 9.23V6.897a.07.07 0 0 1 .028-.061l4.83-2.787a4.5 4.5 0 0 1 6.68 4.66zm-12.64 4.135-2.02-1.164a.08.08 0 0 1-.038-.057V6.075a4.5 4.5 0 0 1 7.375-3.453l-.142.08L8.704 5.46a.8.8 0 0 0-.393.681zm1.097-2.365 2.602-1.5 2.607 1.5v2.999l-2.597 1.5-2.607-1.5Z"/>
                  </svg>
                )}
              </div>
              <span className="settings-source-card-name">{source.display_name}</span>
            </div>
          ))}

          {/* Add Source card */}
          <div className="settings-source-card" onClick={() => setShowAddSource(true)}>
            <div className="settings-source-icon settings-add-icon">
              <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
            </div>
            <span className="settings-source-card-name">Add Source</span>
          </div>
        </div>
        {editingSource && (
          <div className="settings-edit-modal">
            <label>
              <span>Type</span>
              <select value={editingSource.adapter} onChange={e => setEditingSource({...editingSource, adapter: e.target.value})}>
                <option value="pi">Pi</option>
                <option value="claude">Claude Code</option>
                <option value="codex">Codex CLI</option>
              </select>
            </label>
            <label>
              <span>Display name</span>
              <input value={editingSource.display_name} onChange={e => setEditingSource({...editingSource, display_name: e.target.value})} />
            </label>
            <label>
              <span>Session directory</span>
              <input value={editingSource.path} onChange={e => setEditingSource({...editingSource, path: e.target.value})} />
            </label>
            <div className="settings-add-source-actions">
              <button className="settings-action-btn settings-action-primary" onClick={handleSaveSource}>Save</button>
              <button className="settings-action-btn settings-action-danger" onClick={() => { if (confirm('Remove this data source? Entries already imported will be kept.')) { handleRemoveSource(editingSource.id); setEditingSource(null); }}}>Delete</button>
              <button className="settings-action-btn" onClick={() => setEditingSource(null)}>Cancel</button>
            </div>
          </div>
        )}

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


        <div className="settings-data-row">
          <label>
            <span>Pricing override</span>
            <input
              value={settings.model_pricing_path ?? ''}
              onChange={e => update('model_pricing_path', e.target.value || null)}
              placeholder="~/.config/loc-dock/litellm.json"
            />
          </label>
          <button
            className="settings-inline-reset"
            onClick={() => { update('model_pricing_path', null); }}
            title="Use bundled pricing defaults"
          >
            reset
          </button>
        </div>

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
