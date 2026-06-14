import { X, Info, Settings, ScrollText } from "lucide-react";
import { RangeStats, TimeRange, ChartMode } from "../lib/types";
import { fmtCost, fmtLoc } from "../lib/format";

interface Props {
  stats: RangeStats | null;
  range: TimeRange;
  mode: ChartMode;
  onToggleRange: () => void;
  onToggleMode: () => void;
  onSettings: () => void;
  onClose: () => void;
  onShowTooltip: (show: boolean) => void;
  onToggleSummary: () => void;
  summaryVisible: boolean;
}

export function TopRow({ stats, range, mode, onToggleRange, onToggleMode, onSettings, onClose, onShowTooltip, onToggleSummary, summaryVisible }: Props) {
  const s = stats;

  return (
    <div className="top-row">
      <div className="top-row-stats">
        <button className={`icon-btn ${summaryVisible ? "active" : ""}`} onClick={onToggleSummary} title="AI Summary">
          <ScrollText size={14} />
        </button>
        <span className="sep" />
        <span className="loc-add">+{s ? fmtLoc(s.loc_added) : "0"}</span>
        <span className="loc-del">-{s ? fmtLoc(s.loc_deleted) : "0"}</span>
        <span className="sep" />
        <span className="cost">{s ? fmtCost(s.cost_total) : "$0"}</span>
        <span className="info-icon"
          onMouseEnter={() => onShowTooltip(true)}
          onMouseLeave={() => onShowTooltip(false)}>
          <Info size={12} />
        </span>
        <span className="sep hide-narrow" />
        <span className="sessions hide-narrow">
          <span className="sessions-label">S:</span>
          <span className="sessions-active">{s?.sessions_active ?? 0}</span>
          <span className="sessions-sep">/</span>
          <span className="sessions-total">{s?.sessions_total ?? 0}</span>
        </span>
      </div>
      <div className="top-row-controls">
        <button className="toggle-btn" onClick={onToggleRange}>{range.toUpperCase()}</button>
        <button className="toggle-btn" onClick={onToggleMode}>{mode.toUpperCase()}</button>
        <button className="icon-btn" onClick={onSettings}><Settings size={14} /></button>
        <button className="icon-btn" onClick={onClose}><X size={14} /></button>
      </div>
    </div>
  );
}
