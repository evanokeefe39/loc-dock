# LOC Dock Tauri — Open Issues

## Bug: Token and cost chart buckets mostly empty
- **Severity:** High
- **Symptom:** Token chart shows zero bars despite 130M+ tokens in bottom row stats. Cost chart shows only a few bars despite $1,682 total.
- **Root cause:** `parse_ts_offset()` in `data.rs` fails to parse DuckDB timestamp strings. DuckDB `ts::VARCHAR` outputs format like `2026-06-10 14:30:00` but the parser tries `NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S")` which may include fractional seconds or other variations. Most timestamps silently fail to parse, so most buckets stay at zero.
- **Fix:** Log a sample timestamp from DuckDB to see the actual format, then fix the parser. Alternatively, return timestamps as epoch seconds from DuckDB instead of strings.

## Bug: Transparency makes text see-through
- **Severity:** Medium
- **Symptom:** Text behind the widget bleeds through — can read desktop content through the widget text and chart.
- **Root cause:** Using CSS `opacity: var(--alpha)` on `.app` makes the entire element transparent including children. Should use `background: rgba(r,g,b,alpha)` on the container so only the background is semi-transparent, not the text/chart content.
- **Fix:** Convert hex bg color + alpha to `rgba()` in the theme hook and apply as `background-color` instead of `opacity`.

## Bug: Pin dropdown menu clips at window right edge
- **Severity:** Low
- **Symptom:** Menu items truncated when pin button is near the right edge of the window.
- **Status:** Partially fixed with `position: fixed; right: 4px` but needs verification.

## Enhancement: LOC chart shows single bar when commits are clustered
- **Severity:** Low
- **Symptom:** Day view shows one tall bar when all commits happen in a short window.
- **Note:** This is technically correct behavior (all commits land in one bucket), but the visual is misleading. The tkinter version had the same behavior. Could consider wider minimum bar distribution or sub-hour bucketing.
