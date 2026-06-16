import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AllStats } from "../lib/types";

export function useStats() {
  const [stats, setStats] = useState<AllStats | null>(null);

  useEffect(() => {
    const fetch = () =>
      invoke<AllStats>("get_stats")
        .then(setStats)
        .catch(console.error);
    fetch();
    const id = setInterval(fetch, 10_000);
    return () => clearInterval(id);
  }, []);

  return stats;
}
