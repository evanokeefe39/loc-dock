import { useState, useRef, useCallback } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import Markdown from "react-markdown";
import { SummaryData, TimeRange } from "../lib/types";

interface Props {
  summary: SummaryData;
  range: TimeRange;
}

export function SummaryPanel({ summary, range }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [height, setHeight] = useState(100);
  const dragging = useRef(false);
  const startY = useRef(0);
  const startH = useRef(0);

  const text = range === "day" ? summary.day_summary : summary.week_summary;
  const hasContent = text !== null;

  if (!hasContent && !summary.loading && summary.day_commits === 0) return null;

  const teaser = summary.loading
    ? "Summarizing..."
    : `${summary.day_repos} repo${summary.day_repos !== 1 ? "s" : ""} · ${summary.day_commits} commit${summary.day_commits !== 1 ? "s" : ""}`;

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
    <div className={`summary-panel ${expanded ? "expanded" : ""}`}>
      <button
        className="summary-header"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="summary-teaser">{teaser}</span>
        {expanded ? <ChevronDown size={12} /> : <ChevronUp size={12} />}
      </button>
      {expanded && text && (
        <>
          <div className="summary-body" style={{ maxHeight: height }}>
            <Markdown>{text}</Markdown>
          </div>
          <div className="summary-resize" onMouseDown={onResizeStart} />
        </>
      )}
    </div>
  );
}
