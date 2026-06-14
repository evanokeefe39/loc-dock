import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

const BRAILLE = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

interface ActiveTask {
  id: number;
  name: string;
  elapsed_ms: number;
}

export function StatusSpinner() {
  const [tasks, setTasks] = useState<ActiveTask[]>([]);
  const [frame, setFrame] = useState(0);
  const [hovered, setHovered] = useState(false);

  useEffect(() => {
    const refresh = () => {
      invoke<ActiveTask[]>("get_active_tasks").then(setTasks).catch(() => {});
    };
    refresh();
    const unlisten = listen("tasks-changed", refresh);
    return () => { unlisten.then(fn => fn()); };
  }, []);

  useEffect(() => {
    if (tasks.length === 0) return;
    const id = setInterval(() => setFrame(f => (f + 1) % BRAILLE.length), 80);
    return () => clearInterval(id);
  }, [tasks.length]);

  if (tasks.length === 0) return null;

  return (
    <div
      className="status-spinner"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <span className="spinner-char">{BRAILLE[frame]}</span>
      {hovered && (
        <div className="spinner-tooltip">
          {tasks.map(t => <div key={t.id}>{t.name}</div>)}
        </div>
      )}
    </div>
  );
}
