import { useState } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { SummaryData, TimeRange } from "../lib/types";

interface Props {
  summary: SummaryData;
  range: TimeRange;
}

export function SummaryPanel({ summary, range }: Props) {
  const [expanded, setExpanded] = useState(false);

  const text = range === "day" ? summary.day_summary : summary.week_summary;
  const hasContent = text !== null;

  if (!hasContent && !summary.loading) return null;

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
