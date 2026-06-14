import { useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { SummaryData, TimeRange } from "../lib/types";

interface Props {
  summary: SummaryData;
  range: TimeRange;
  onSettings: () => void;
}

export function SummaryPanel({ summary, range, onSettings }: Props) {
  const [expanded, setExpanded] = useState(false);

  const text = range === "day" ? summary.day_summary : summary.week_summary;
  const hasContent = text !== null;

  if (summary.no_api_key) {
    return (
      <div className="summary-panel summary-setup">
        <button className="summary-header" onClick={onSettings}>
          <span className="summary-teaser summary-setup-msg">
            Add LLM API key in Settings for AI commit summaries
          </span>
        </button>
      </div>
    );
  }

  if (!hasContent && !summary.loading && summary.day_commits === 0) return null;

  const teaser = summary.loading
    ? "Summarizing..."
    : `${summary.day_repos} repo${summary.day_repos !== 1 ? "s" : ""} · ${summary.day_commits} commit${summary.day_commits !== 1 ? "s" : ""}`;

  return (
    <div className={`summary-panel ${expanded ? "expanded" : ""}`}>
      <button
        className="summary-header"
        onClick={() => setExpanded(!expanded)}
      >
        <span className="summary-teaser">{teaser}</span>
        {expanded ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
      </button>
      {expanded && text && (
        <div className="summary-body">
          {text}
        </div>
      )}
    </div>
  );
}
