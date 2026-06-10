# LOC Dock Tauri — Open Issues

## Bug: Token and cost chart Y-axis wrong, most data missing from buckets
- **Severity:** High
- **Symptom:** Cost chart Y-axis shows $1.00 max when total is $1,682. Token chart shows 73K max when total is 130M+. Some bars render but most data is missing from the bucketed arrays.
- **Root cause:** `parse_ts_offset()` in `data.rs` silently fails to parse most DuckDB timestamp strings. DuckDB `ts::VARCHAR` likely outputs fractional seconds or a format that `NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S")` can't handle. Most timestamps fail to parse, so most data points are dropped during bucketing. Bottom row stats are correct because they come directly from DuckDB aggregation (query_since), not from the broken bucket functions.
- **Fix:** Log a sample timestamp from DuckDB to see the actual format, then fix the parser. Alternatively, return timestamps as epoch seconds from DuckDB instead of strings to avoid parsing entirely.

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
