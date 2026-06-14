import { useState, useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

const BRAILLE = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

export function StatusSpinner() {
  const [jobs, setJobs] = useState<string[]>([]);
  const [frame, setFrame] = useState(0);
  const [hovered, setHovered] = useState(false);
  const firstLoad = useRef(false);

  useEffect(() => {
    const unlisten = listen<string>("status-update", (event) => {
      const msg = event.payload;
      const isComplete = msg.startsWith("Refreshed in") || msg.startsWith("Summary cycle:") || msg === "Data refreshed";
      if (isComplete) {
        firstLoad.current = true;
        setJobs(prev => prev.filter(j => j !== msg));
      } else {
        setJobs(prev => {
          const next = prev.filter(j => j !== msg);
          next.push(msg);
          return next;
        });
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  useEffect(() => {
    if (jobs.length === 0) return;
    const id = setInterval(() => setFrame(f => (f + 1) % BRAILLE.length), 80);
    return () => clearInterval(id);
  }, [jobs.length]);

  if (!firstLoad.current || jobs.length === 0) return null;

  return (
    <div
      className="status-spinner"
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <span className="spinner-char">{BRAILLE[frame]}</span>
      {hovered && (
        <div className="spinner-tooltip">
          {jobs.map((j, i) => <div key={i}>{j}</div>)}
        </div>
      )}
    </div>
  );
}
