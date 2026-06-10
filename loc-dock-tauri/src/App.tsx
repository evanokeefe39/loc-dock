import { useState, useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { useStats } from "./hooks/useStats";
import { useTheme } from "./hooks/useTheme";
import { TopRow } from "./components/TopRow";
import { Chart } from "./components/Chart";
import { BottomRow } from "./components/BottomRow";
import { CostTooltip } from "./components/CostTooltip";
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

  useEffect(() => {
    if (!shown.current && theme) {
      shown.current = true;
      invoke("snap_to_corner", { corner: "bottom-right" }).then(() => {
        getCurrentWindow().show();
      }).catch(() => {
        getCurrentWindow().show();
      });
    }
  }, [theme]);

  const currentStats = stats ? stats[range] : null;
  const MODES: ChartMode[] = ["loc", "cost", "tokens"];

  const handleSnapTo = (corner: Corner) => {
    invoke("snap_to_corner", { corner });
  };

  const handleClose = () => getCurrentWindow().close();

  return (
    <div className="app" data-tauri-drag-region>
      <TopRow
        stats={currentStats}
        range={range}
        mode={mode}
        onToggleRange={() => setRange(r => r === "day" ? "week" : "day")}
        onToggleMode={() => setMode(m => MODES[(MODES.indexOf(m) + 1) % MODES.length])}
        onSnapTo={handleSnapTo}
        onClose={handleClose}
        onShowTooltip={setTooltipVisible}
      />
      <Chart stats={stats} mode={mode} range={range} theme={theme} />
      <BottomRow tokens={currentStats?.tokens ?? null} loading={!stats} />
      <CostTooltip breakdown={currentStats?.cost_breakdown ?? null} visible={tooltipVisible} />
    </div>
  );
}

export default App;
