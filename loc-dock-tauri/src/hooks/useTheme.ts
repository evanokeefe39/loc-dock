import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Theme } from "../lib/types";

const DEFAULT_THEME: Theme = {
  alpha: 0.92, bg: "#1a1a2e", chart_bg: "#12121f", tooltip_bg: "#222244",
  text: "#e0e0e0", text_dim: "#6b7280", axis: "#333350",
  loc_add: "#34d399", loc_del: "#ef4444", cost: "#a78bfa",
  sessions: "#f97316", tok_input: "#e0e0e0", tok_output: "#f472b6",
  tok_cache_write: "#facc15", tok_cache_read: "#38bdf8",
};

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(DEFAULT_THEME);

  useEffect(() => {
    invoke<Theme>("get_theme").then((t) => {
      setTheme(t);
      const root = document.documentElement;
      for (const [key, value] of Object.entries(t)) {
        if (key === "alpha") {
          root.style.setProperty("--alpha", String(value));
          continue;
        }
        root.style.setProperty(`--${key.replace(/_/g, "-")}`, value as string);
      }
    }).catch(console.error);
  }, []);

  return theme;
}
