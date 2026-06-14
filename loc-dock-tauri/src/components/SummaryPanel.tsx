import { useState, useRef, useCallback } from "react";
import Markdown from "react-markdown";
import { SummaryData, TimeRange } from "../lib/types";

interface Props {
  summary: SummaryData;
  range: TimeRange;
}

export function SummaryPanel({ summary, range }: Props) {
  const [height, setHeight] = useState(100);
  const dragging = useRef(false);
  const startY = useRef(0);
  const startH = useRef(0);

  const text = range === "day" ? summary.day_summary : summary.week_summary;

  const label = range === "day" ? "Today" : "This week";
  const teaser = summary.loading
    ? "Summarizing..."
    : summary.day_commits > 0
      ? `${label} · ${summary.day_repos} repo${summary.day_repos !== 1 ? "s" : ""} · ${summary.day_commits} commit${summary.day_commits !== 1 ? "s" : ""}`
      : `${label} · No commits yet`;

  const onResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    startY.current = e.clientY;
    startH.current = height;

    const onMove = (ev: MouseEvent) => {
      if (!dragging.current) return;
      const delta = ev.clientY - startY.current;
      setHeight(Math.max(40, Math.min(300, startH.current + delta)));
    };
    const onUp = () => {
      dragging.current = false;
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }, [height]);

  return (
    <div className="summary-panel">
      <div className="summary-label">{teaser}</div>
      <div className="summary-body" style={{ maxHeight: height }}>
        {text
          ? <Markdown>{text}</Markdown>
          : <span className="summary-empty">
              {summary.no_api_key
                ? "Configure LLM API key in Settings to enable AI summaries"
                : summary.loading ? "Generating summary..." : "Waiting for commits..."}
            </span>
        }
      </div>
      <div className="summary-resize" onMouseDown={onResizeStart} />
    </div>
  );
}
