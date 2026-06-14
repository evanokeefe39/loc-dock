import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SummaryData } from "../lib/types";

const DEFAULT: SummaryData = {
  day_summary: null,
  week_summary: null,
  day_repos: 0,
  day_commits: 0,
  loading: false,
};

export function useSummary() {
  const [summary, setSummary] = useState<SummaryData>(DEFAULT);

  useEffect(() => {
    invoke<SummaryData>("get_summary").then(setSummary).catch(() => {});
    const unlisten = listen<SummaryData>("summary-update", (event) => {
      setSummary(event.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  return summary;
}
