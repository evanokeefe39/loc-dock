import { useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { useStatsQuery, useSummaryQuery, useThemeQuery, applyTheme, DEFAULT_THEME } from "./hooks/queries";
import { SummaryPanel } from "./components/SummaryPanel";
import { NotificationBanner } from "./components/NotificationBanner";
import { TopRow } from "./components/TopRow";
import { Chart } from "./components/Chart";
import { BottomRow } from "./components/BottomRow";
import { CostTooltip } from "./components/CostTooltip";
import { SettingsPanel } from "./components/SettingsPanel";
import { ResizeBorders } from "./components/ResizeBorders";
import { StatusSpinner } from "./components/Toast";
import { useUIStore } from "./lib/store";
import "./styles/global.css";

function App() {
  const { data: stats, isLoading } = useStatsQuery();
  const { data: summary } = useSummaryQuery();
  const { data: theme } = useThemeQuery();
  const shown = useRef(false);

  const {
    range, mode, tooltipVisible, settingsOpen, summaryOpen, hideNoPrs,
    toggleRange, toggleMode, setTooltipVisible, setSettingsOpen,
    setSummaryOpen, setHideNoPrs,
  } = useUIStore();

  // Apply theme when it loads
  useEffect(() => {
    if (theme) applyTheme(theme);
  }, [theme]);

  // Signal to Rust that React has mounted, so it can show the window.
  // The window was positioned (bottom-right) in Rust setup() but kept
  // hidden to avoid WebView2 navigation errors. We also set the initial
  // size and snap to corner here.
  useEffect(() => {
    if (shown.current) return;
    shown.current = true;
    getCurrentWindow()
      .setSize(new LogicalSize(500, 340))
      .then(() => invoke("snap_to_corner", { corner: "bottom-right" }))
      .catch(() => {});
    emit("frontend-ready", {}).catch(() => {});
  }, []);

  // Fetch hideNoPrs setting once on mount (setting is stable during a session)
  useEffect(() => {
    invoke<{ hide_repos_without_prs: boolean }>("get_settings")
      .then(s => setHideNoPrs(s.hide_repos_without_prs))
      .catch(() => {});
  }, [setHideNoPrs]);

  // Listen for open-settings event
  useEffect(() => {
    const unlisten = listen("open-settings", () => setSettingsOpen(true));
    return () => { unlisten.then(f => f()); };
  }, [setSettingsOpen]);

  const currentStats = stats ? stats[range] : null;
  const handleClose = () => getCurrentWindow().hide();

  return (
    <div className="app" data-tauri-drag-region>
      <TopRow
        stats={currentStats}
        ready={!isLoading}
        range={range}
        mode={mode}
        onToggleRange={toggleRange}
        onToggleMode={toggleMode}
        onSettings={() => setSettingsOpen(true)}
        onClose={handleClose}
        onShowTooltip={setTooltipVisible}
        onToggleSummary={() => setSummaryOpen(!summaryOpen)}
        summaryVisible={summaryOpen}
      />
      <NotificationBanner visible={summary?.no_api_key ?? false} onSettings={() => setSettingsOpen(true)} />
      {summaryOpen && <SummaryPanel summary={summary} range={range} hideNoPrs={hideNoPrs} />}
      <Chart stats={stats ?? null} mode={mode} range={range} theme={theme ?? DEFAULT_THEME} />
      <BottomRow tokens={currentStats?.tokens ?? null} />
      <CostTooltip breakdown={currentStats?.cost_breakdown ?? null} visible={tooltipVisible} />
      <SettingsPanel visible={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <ResizeBorders />
      <StatusSpinner />
    </div>
  );
}

export default App;
