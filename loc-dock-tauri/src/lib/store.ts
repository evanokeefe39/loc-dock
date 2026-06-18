import { create } from "zustand";
import { TimeRange, ChartMode } from "./types";

const RANGES: TimeRange[] = ["day", "week", "month", "year"];
const MODES: ChartMode[] = ["loc", "cost", "tokens"];

interface UIState {
  range: TimeRange;
  mode: ChartMode;
  tooltipVisible: boolean;
  settingsOpen: boolean;
  summaryOpen: boolean;
  hideNoPrs: boolean;
  setRange: (r: TimeRange) => void;
  setMode: (m: ChartMode) => void;
  toggleRange: () => void;
  toggleMode: () => void;
  setTooltipVisible: (v: boolean) => void;
  setSettingsOpen: (v: boolean) => void;
  setSummaryOpen: (v: boolean) => void;
  setHideNoPrs: (v: boolean) => void;
}

export const useUIStore = create<UIState>((set) => ({
  range: "day",
  mode: "loc",
  tooltipVisible: false,
  settingsOpen: false,
  summaryOpen: false,
  hideNoPrs: false,
  setRange: (r) => set({ range: r }),
  setMode: (m) => set({ mode: m }),
  toggleRange: () => set((s) => ({ range: RANGES[(RANGES.indexOf(s.range) + 1) % RANGES.length] })),
  toggleMode: () => set((s) => ({ mode: MODES[(MODES.indexOf(s.mode) + 1) % MODES.length] })),
  setTooltipVisible: (v) => set({ tooltipVisible: v }),
  setSettingsOpen: (v) => set({ settingsOpen: v }),
  setSummaryOpen: (v) => set({ summaryOpen: v }),
  setHideNoPrs: (v) => set({ hideNoPrs: v }),
}));
