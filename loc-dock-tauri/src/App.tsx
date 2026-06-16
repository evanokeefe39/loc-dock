import { useState, useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useStats } from "./hooks/useStats";
import { useSummary } from "./hooks/useSummary";
import { useTheme } from "./hooks/useTheme";
import { SummaryPanel } from "./components/SummaryPanel";
import { NotificationBanner } from "./components/NotificationBanner";
import { TopRow } from "./components/TopRow";
import { Chart } from "./components/Chart";
import { BottomRow } from "./components/BottomRow";
import { CostTooltip } from "./components/CostTooltip";
import { SettingsPanel } from "./components/SettingsPanel";
import { ResizeBorders } from "./components/ResizeBorders";
import { StatusSpinner } from "./components/Toast";
import { TimeRange, ChartMode } from "./lib/types";
import "./styles/global.css";

function App() {
  const stats = useStats();
  const summary = useSummary();
  const theme = useTheme();
  const shown = useRef(false);

  const [range, setRange] = useState<TimeRange>("day");
  const [mode, setMode] = useState<ChartMode>("loc");
  const [tooltipVisible, setTooltipVisible] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const [hideNoPrs, setHideNoPrs] = useState(false);

  // The Rust side already shows + positions the window at startup (decoupled from
  // the frontend). Here we just ensure size/corner once mounted — idempotent, same
  // position, so no flash. show() is a belt-and-suspenders fallback, not gated on
  // theme or data.
  useEffect(() => {
    if (shown.current) return;
    shown.current = true;
    const win = getCurrentWindow();
    win.show().catch(() => {});
    win.setSize(new LogicalSize(500, 340))
      .then(() => invoke("snap_to_corner", { corner: "bottom-right" }))
      .catch(() => {});
  }, []);

  useEffect(() => {
    invoke<{ hide_repos_without_prs: boolean }>("get_settings")
      .then(s => setHideNoPrs(s.hide_repos_without_prs))
      .catch(() => {});
  }, [settingsOpen]);

  useEffect(() => {
    const unlisten = listen("open-settings", () => setSettingsOpen(true));
    return () => { unlisten.then(f => f()); };
  }, []);

  const currentStats = stats ? stats[range] : null;
  const MODES: ChartMode[] = ["loc", "cost", "tokens"];
  const RANGES: TimeRange[] = ["day", "week", "month", "year"];

  const handleClose = () => getCurrentWindow().hide();

  return (
    <div className="app" data-tauri-drag-region>
      <TopRow
        stats={currentStats}
        range={range}
        mode={mode}
        onToggleRange={() => setRange(r => RANGES[(RANGES.indexOf(r) + 1) % RANGES.length])}
        onToggleMode={() => setMode(m => MODES[(MODES.indexOf(m) + 1) % MODES.length])}
        onSettings={() => setSettingsOpen(true)}
        onClose={handleClose}
        onShowTooltip={setTooltipVisible}
        onToggleSummary={() => setSummaryOpen(v => !v)}
        summaryVisible={summaryOpen}
      />
      <NotificationBanner visible={summary.no_api_key} onSettings={() => setSettingsOpen(true)} />
      {summaryOpen && <SummaryPanel summary={summary} range={range} hideNoPrs={hideNoPrs} />}
      <Chart stats={stats} mode={mode} range={range} theme={theme} />
      <BottomRow tokens={currentStats?.tokens ?? null} />
      <CostTooltip breakdown={currentStats?.cost_breakdown ?? null} visible={tooltipVisible} />
      <SettingsPanel visible={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <ResizeBorders />
      <StatusSpinner />
    </div>
  );
}

export default App;
