import { useState, useRef, useCallback, ReactNode } from "react";
import { SummaryData, TimeRange } from "../lib/types";

const REF_PATTERN = /(#\d+|[A-Z][A-Z0-9]+-\d+)/g;

function formatHighlight(text: string): ReactNode {
  const parts = text.split(REF_PATTERN);
  if (parts.length === 1) return text;
  return parts.map((part, i) =>
    REF_PATTERN.test(part)
      ? <span key={i} className="highlight-ref">{part}</span>
      : part
  );
}

interface Props {
  summary: SummaryData;
  range: TimeRange;
}

export function SummaryPanel({ summary, range }: Props) {
  const [height, setHeight] = useState(100);
  const dragging = useRef(false);
  const startY = useRef(0);
  const startH = useRef(0);

  const repos = range === "day" ? summary.day_repos : summary.week_repos;
  const repoCount = range === "day" ? summary.day_repo_count : summary.week_repo_count;
  const commits = range === "day" ? summary.day_commits : summary.week_commits;
  const prs = range === "day" ? summary.day_prs : summary.week_prs;

  const hasRepos = repos.length > 0;

  const onResizeStart = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    startY.current = e.clientY;
    startH.current = height;

    const onMove = (ev: MouseEvent) => {
      if (!dragging.current) return;
      const delta = ev.clientY - startY.current;
      setHeight(Math.max(40, startH.current + delta));
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
        {commits > 0
          ? `${repoCount} repo${repoCount !== 1 ? "s" : ""} · ${commits} commit${commits !== 1 ? "s" : ""}${prs > 0 ? ` · ${prs} PR${prs !== 1 ? "s" : ""}` : ""}`
          : range === "day" ? "No commits yet today" : "No commits this week"}
      </div>
      <div className="summary-body" style={{ height }}>
        {summary.loading ? (
          <span className="summary-empty">AI summary is being generated...</span>
        ) : summary.no_api_key ? (
          <span className="summary-empty">Configure LLM API key in Settings to enable AI summaries</span>
        ) : !hasRepos ? (
          <span className="summary-empty">{range === "day" ? "No commits yet today" : "No commits this week"}</span>
        ) : (
          <div className="summary-cards">
            {repos.map(repo => (
              <div key={repo.name} className="summary-card">
                <div className="card-header">
                  <span className="card-name">{repo.name}</span>
                  <span className="card-count">{repo.commits}</span>
                </div>
                {repo.highlights.length > 0 && (
                  <ul className="card-highlights">
                    {repo.highlights.map((h, i) => (
                      <li key={i}>{formatHighlight(h)}</li>
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
