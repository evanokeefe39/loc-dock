import { Pin, X, Info } from "lucide-react";
import { RangeStats, TimeRange, ChartMode } from "../lib/types";
import { fmtCost, fmtLoc } from "../lib/format";

interface Props {
  stats: RangeStats | null;
  range: TimeRange;
  mode: ChartMode;
  pinned: boolean;
  onToggleRange: () => void;
  onToggleMode: () => void;
  onPin: () => void;
  onClose: () => void;
  onShowTooltip: (show: boolean) => void;
}

export function TopRow({ stats, range, mode, pinned, onToggleRange, onToggleMode, onPin, onClose, onShowTooltip }: Props) {
  const s = stats;
  return (
    <div className="top-row" data-tauri-drag-region>
      <span className="loc-add">+{s ? fmtLoc(s.loc_added) : "0"}</span>
      <span className="loc-del">-{s ? fmtLoc(s.loc_deleted) : "0"}</span>
      <span className="sep" />
      <span className="cost">{s ? fmtCost(s.cost_total) : "$0"}</span>
      <span className="info-icon"
        onMouseEnter={() => onShowTooltip(true)}
        onMouseLeave={() => onShowTooltip(false)}>
        <Info size={14} />
      </span>
      <span className="sep" />
      <span className="sessions-label">S:</span>
      <span className="sessions-active">{s?.sessions_active ?? 0}</span>
      <span className="sessions-sep">/</span>
      <span className="sessions-total">{s?.sessions_total ?? 0}</span>
      <div className="spacer" />
      <button className="toggle-btn" onClick={onToggleRange}>{range.toUpperCase()}</button>
      <button className="toggle-btn" onClick={onToggleMode}>{mode.toUpperCase()}</button>
      <span className="sep" />
      {!pinned && (
        <button className="icon-btn" onClick={onPin}><Pin size={14} /></button>
      )}
      <button className="icon-btn" onClick={onClose}><X size={14} /></button>
    </div>
  );
}
