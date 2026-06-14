import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SummaryData } from "../lib/types";

const DEFAULT: SummaryData = {
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
