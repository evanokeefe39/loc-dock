import { useState } from "react";
import { useStats } from "./hooks/useStats";
import { useTheme } from "./hooks/useTheme";
import { useWindowControls } from "./hooks/useWindowControls";
import { TopRow } from "./components/TopRow";
import { Chart } from "./components/Chart";
import { BottomRow } from "./components/BottomRow";
import { CostTooltip } from "./components/CostTooltip";
import { TimeRange, ChartMode } from "./lib/types";
import "./styles/global.css";

function App() {
  const stats = useStats();
  const theme = useTheme();
  const { pinned, close, snapToCorner } = useWindowControls();

  const [range, setRange] = useState<TimeRange>("day");
  const [mode, setMode] = useState<ChartMode>("loc");
  const [tooltipVisible, setTooltipVisible] = useState(false);

  const currentStats = stats ? stats[range] : null;
  const MODES: ChartMode[] = ["loc", "cost", "tokens"];

  return (
    <div className="app">
      <TopRow
        stats={currentStats}
        range={range}
        mode={mode}
        pinned={pinned}
        onToggleRange={() => setRange(r => r === "day" ? "week" : "day")}
        onToggleMode={() => setMode(m => MODES[(MODES.indexOf(m) + 1) % MODES.length])}
        onPin={snapToCorner}
        onClose={close}
        onShowTooltip={setTooltipVisible}
      />
      <Chart stats={stats} mode={mode} range={range} theme={theme} />
      <BottomRow tokens={currentStats?.tokens ?? null} />
      <CostTooltip breakdown={currentStats?.cost_breakdown ?? null} visible={tooltipVisible} />
    </div>
  );
}

export default App;
