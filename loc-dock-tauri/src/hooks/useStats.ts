import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AllStats } from "../lib/types";

export function useStats() {
  const [stats, setStats] = useState<AllStats | null>(null);

  useEffect(() => {
    invoke<AllStats>("get_stats").then(setStats).catch(console.error);
    const unlisten = listen<AllStats>("stats-update", (event) => {
      setStats(event.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  return stats;
}
