import { useState, useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useStats } from "./hooks/useStats";
import { useTheme } from "./hooks/useTheme";
import { TopRow } from "./components/TopRow";
import { Chart } from "./components/Chart";
import { BottomRow } from "./components/BottomRow";
import { CostTooltip } from "./components/CostTooltip";
import { SettingsPanel } from "./components/SettingsPanel";
import { ResizeBorders } from "./components/ResizeBorders";
import { TimeRange, ChartMode } from "./lib/types";
import "./styles/global.css";

type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";

function App() {
  const stats = useStats();
  const theme = useTheme();
  const shown = useRef(false);

  const [range, setRange] = useState<TimeRange>("day");
  const [mode, setMode] = useState<ChartMode>("loc");
  const [tooltipVisible, setTooltipVisible] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    if (!shown.current && theme) {
      shown.current = true;
      const win = getCurrentWindow();
      win.setSize(new LogicalSize(420, 340))
        .then(() => invoke("snap_to_corner", { corner: "bottom-right" }))
        .then(() => win.show())
        .catch(() => win.show());
    }
  }, [theme]);

  useEffect(() => {
    const unlisten = listen("open-settings", () => setSettingsOpen(true));
    return () => { unlisten.then(f => f()); };
  }, []);

  const currentStats = stats ? stats[range] : null;
  const MODES: ChartMode[] = ["loc", "cost", "tokens"];

  const handleSnapTo = (corner: Corner) => {
    invoke("snap_to_corner", { corner });
  };

  const handleClose = () => getCurrentWindow().hide();

  return (
    <div className="app" data-tauri-drag-region>
      <TopRow
        stats={currentStats}
        range={range}
        mode={mode}
        onToggleRange={() => setRange(r => r === "day" ? "week" : "day")}
        onToggleMode={() => setMode(m => MODES[(MODES.indexOf(m) + 1) % MODES.length])}
        onSnapTo={handleSnapTo}
        onSettings={() => setSettingsOpen(true)}
        onClose={handleClose}
        onShowTooltip={setTooltipVisible}
      />
      <Chart stats={stats} mode={mode} range={range} theme={theme} />
      <BottomRow tokens={currentStats?.tokens ?? null} />
      <CostTooltip breakdown={currentStats?.cost_breakdown ?? null} visible={tooltipVisible} />
      <SettingsPanel visible={settingsOpen} onClose={() => setSettingsOpen(false)} />
      <ResizeBorders />
    </div>
  );
}

export default App;
