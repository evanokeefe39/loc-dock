import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Theme } from "../lib/types";

const DEFAULT_THEME: Theme = {
  alpha: 0.92, bg: "#202020", chart_bg: "#181818", tooltip_bg: "#2a2a2a",
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
        if (key === "alpha") continue;
        root.style.setProperty(`--${key.replace(/_/g, "-")}`, value as string);
      }
      const hex = t.bg;
      const r = parseInt(hex.slice(1, 3), 16);
      const g = parseInt(hex.slice(3, 5), 16);
      const b = parseInt(hex.slice(5, 7), 16);
      root.style.setProperty("--bg-alpha", `rgba(${r},${g},${b},${t.alpha})`);

    }).catch(console.error);
  }, []);

  return theme;
}
