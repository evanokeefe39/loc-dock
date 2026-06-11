import { useState, useRef, useEffect } from "react";
import { Pin, X, Info, Settings } from "lucide-react";
import { RangeStats, TimeRange, ChartMode } from "../lib/types";
import { fmtCost, fmtLoc } from "../lib/format";

type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";

interface Props {
  stats: RangeStats | null;
  range: TimeRange;
  mode: ChartMode;
  onToggleRange: () => void;
  onToggleMode: () => void;
  onSnapTo: (corner: Corner) => void;
  onSettings: () => void;
  onClose: () => void;
  onShowTooltip: (show: boolean) => void;
}

export function TopRow({ stats, range, mode, onToggleRange, onToggleMode, onSnapTo, onSettings, onClose, onShowTooltip }: Props) {
  const s = stats;
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menuOpen) return;
    const close = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [menuOpen]);

  return (
    <div className="top-row">
      <span className="loc-add">+{s ? fmtLoc(s.loc_added) : "0"}</span>
      <span className="loc-del">-{s ? fmtLoc(s.loc_deleted) : "0"}</span>
      <span className="sep" />
      <span className="cost">{s ? fmtCost(s.cost_total) : "$0"}</span>
      <span className="info-icon"
        onMouseEnter={() => onShowTooltip(true)}
        onMouseLeave={() => onShowTooltip(false)}>
        <Info size={12} />
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
      <div className="pin-container" ref={menuRef}>
        <button className="icon-btn" onClick={() => setMenuOpen(!menuOpen)}>
          <Pin size={12} />
        </button>
        {menuOpen && (
          <div className="pin-menu">
            <button onClick={() => { onSnapTo("top-left"); setMenuOpen(false); }}>↖ Top Left</button>
            <button onClick={() => { onSnapTo("top-right"); setMenuOpen(false); }}>↗ Top Right</button>
            <button onClick={() => { onSnapTo("bottom-left"); setMenuOpen(false); }}>↙ Bot Left</button>
            <button onClick={() => { onSnapTo("bottom-right"); setMenuOpen(false); }}>↘ Bot Right</button>
          </div>
        )}
      </div>
      <button className="icon-btn" onClick={onSettings}><Settings size={12} /></button>
      <button className="icon-btn" onClick={onClose}><X size={12} /></button>
    </div>
  );
}
