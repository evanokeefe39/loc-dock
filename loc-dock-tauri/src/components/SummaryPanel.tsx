import { useState, useRef, useCallback } from "react";
import { SummaryData } from "../lib/types";

interface Props {
  summary: SummaryData;
}

export function SummaryPanel({ summary }: Props) {
  const [height, setHeight] = useState(100);
  const dragging = useRef(false);
  const startY = useRef(0);
  const startH = useRef(0);

  const hasRepos = summary.repos.length > 0;
  const hasHighlights = summary.repos.some(r => r.highlights.length > 0);

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
      <div className="summary-label">
        {summary.day_commits > 0
          ? `${summary.day_repos} repo${summary.day_repos !== 1 ? "s" : ""} · ${summary.day_commits} commit${summary.day_commits !== 1 ? "s" : ""}`
          : "No commits yet today"}
      </div>
      <div className="summary-body" style={{ maxHeight: height }}>
        {summary.loading && !hasHighlights ? (
          <span className="summary-empty">AI summary is being generated...</span>
        ) : summary.no_api_key ? (
          <span className="summary-empty">Configure LLM API key in Settings to enable AI summaries</span>
        ) : !hasRepos ? (
          <span className="summary-empty">No commits yet today</span>
        ) : (
          <div className="summary-cards">
            {summary.repos.map(repo => (
              <div key={repo.name} className="summary-card">
                <div className="card-header">
                  <span className="card-name">{repo.name}</span>
                  <span className="card-count">{repo.commits}</span>
                </div>
                {repo.highlights.length > 0 && (
                  <ul className="card-highlights">
                    {repo.highlights.map((h, i) => (
                      <li key={i}>{h}</li>
                    ))}
                  </ul>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
      <div className="summary-resize" onMouseDown={onResizeStart} />
    </div>
  );
}
