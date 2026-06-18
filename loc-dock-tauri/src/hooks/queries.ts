import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { AllStats, SummaryData, Theme } from "../lib/types";

const DEFAULT_SUMMARY: SummaryData = {
  day_repos: [],
  day_repo_count: 0,
  day_commits: 0,
  day_prs: 0,
  week_repos: [],
  week_repo_count: 0,
  week_commits: 0,
  week_prs: 0,
  loading: false,
  no_api_key: false,
};

export const DEFAULT_THEME: Theme = {
  alpha: 0.92, bg: "#202020", chart_bg: "#181818", tooltip_bg: "#2a2a2a",
  text: "#e0e0e0", text_dim: "#6b7280", axis: "#333350",
  loc_add: "#34d399", loc_del: "#ef4444", cost: "#a78bfa",
  sessions: "#f97316", tok_input: "#e0e0e0", tok_output: "#f472b6",
  tok_cache_write: "#facc15", tok_cache_read: "#38bdf8",
};

export function useStatsQuery() {
  return useQuery({
    queryKey: ["stats"],
    queryFn: () => invoke<AllStats>("get_stats"),
    refetchInterval: 10_000,
  });
}

export function useSummaryQuery() {
  const queryClient = useQueryClient();

  useEffect(() => {
    const unlisten = listen<SummaryData>("summary-update", (event) => {
      queryClient.setQueryData(["summary"], event.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, [queryClient]);

  return useQuery({
    queryKey: ["summary"],
    queryFn: () => invoke<SummaryData>("get_summary").catch(() => DEFAULT_SUMMARY),
    refetchInterval: 10_000,
    initialData: DEFAULT_SUMMARY,
  });
}

export function useThemeQuery() {
  return useQuery({
    queryKey: ["theme"],
    queryFn: () => invoke<Theme>("get_theme"),
    staleTime: Infinity,
  });
}

export function applyTheme(t: Theme) {
  const root = document.documentElement;
  for (const [key, value] of Object.entries(t)) {
    if (key === "alpha") continue;
    root.style.setProperty(`--${key.replace(/_/g, "-")}`, value as string);
  }
  const hexToRgba = (hex: string, a: number) => {
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    return `rgba(${r},${g},${b},${a})`;
  };
  root.style.setProperty("--bg-alpha", hexToRgba(t.bg, t.alpha));
  root.style.setProperty("--chart-bg-alpha", hexToRgba(t.chart_bg, t.alpha));
}
