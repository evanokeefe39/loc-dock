# /// script
# requires-python = ">=3.10"
# dependencies = ["duckdb", "tzdata"]
# ///
"""
LOC Dock — floating desktop widget for daily dev metrics.

Run:  uv run dock.py
"""

import logging
import os
import subprocess
import threading
import tkinter as tk
from datetime import datetime, timezone, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo

import duckdb

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("loc-dock")

# ── Config ────────────────────────────────────────────────────────────
REFRESH_UI_MS = 30_000
REFRESH_DATA_MS = 60_000
CLAUDE_DIR = Path(os.environ.get("LOCDOCK_CLAUDE_DIR", Path.home() / ".claude"))
PROJECTS_DIR = CLAUDE_DIR / "projects"
REPOS_DIR = Path(os.environ.get("LOCDOCK_REPOS_DIR", Path.home() / "repos"))
TZ = ZoneInfo(os.environ.get("LOCDOCK_TIMEZONE", "Europe/Berlin"))
DAY_START_HOUR = int(os.environ.get("LOCDOCK_DAY_START_HOUR", "7"))

PRICING = {
    "input":        15.00,
    "output":       75.00,
    "cache_write":  18.75,
    "cache_read":    1.50,
}

# ── Colours ──────────────────────────────────────────────────────────
BG       = "#1a1a2e"
FG       = "#e0e0e0"
ACCENT2  = "#a78bfa"
DIM      = "#6b7280"
GREEN    = "#34d399"
RED      = "#ef4444"
ORANGE   = "#f97316"
PINK     = "#f472b6"
YELLOW   = "#facc15"
BLUE     = "#38bdf8"
CHART_BG = "#12121f"
AXIS_CLR = "#333350"


# ── Git LOC with timestamps ─────────────────────────────────────────
def get_git_loc_timeline(repos_dir: Path, since: datetime) -> list[tuple[datetime, int, int]]:
    """Return list of (commit_time, added, deleted) across all repos since `since`."""
    points: list[tuple[datetime, int, int]] = []
    if not repos_dir.exists():
        return points

    since_iso = since.strftime("%Y-%m-%dT%H:%M:%S%z")

    for entry in repos_dir.iterdir():
        if not entry.is_dir() or not (entry / ".git").exists():
            continue
        try:
            si = subprocess.STARTUPINFO()
            si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            si.wShowWindow = 0
            result = subprocess.run(
                ["git", "log", f"--since={since_iso}", "--format=%aI", "--numstat"],
                capture_output=True, text=True, cwd=str(entry), timeout=10,
                startupinfo=si, creationflags=subprocess.CREATE_NO_WINDOW,
            )
            if result.returncode != 0:
                continue

            current_time = None
            for line in result.stdout.splitlines():
                line = line.strip()
                if not line:
                    continue
                if line[0] in "0123456789" and "\t" in line:
                    if current_time is None:
                        continue
                    parts = line.split("\t")
                    if len(parts) >= 2 and parts[0] != "-" and parts[1] != "-":
                        a, d = int(parts[0]), int(parts[1])
                        points.append((current_time, a, d))
                else:
                    try:
                        current_time = datetime.fromisoformat(line)
                    except ValueError:
                        pass
        except (subprocess.TimeoutExpired, OSError):
            continue

    points.sort(key=lambda p: p[0])
    return points


def bucket_timeline(points: list[tuple[datetime, int, int]], since: datetime, until: datetime, n_buckets: int = 32) -> list[tuple[int, int]]:
    total_seconds = (until - since).total_seconds() or 1
    buckets: list[tuple[int, int]] = [(0, 0)] * n_buckets

    for ts, added, deleted in points:
        ts_local = ts.astimezone(TZ)
        offset = (ts_local - since).total_seconds()
        idx = int((offset / total_seconds) * n_buckets)
        idx = max(0, min(n_buckets - 1, idx))
        a, d = buckets[idx]
        buckets[idx] = (a + added, d + deleted)

    return buckets


# ── DuckDB token store ───────────────────────────────────────────────
class UsageStore:
    RETENTION_DAYS = 7

    def __init__(self, projects_dir: Path):
        self.projects_dir = projects_dir
        self._con = duckdb.connect(":memory:")
        self._last_max_mtime: float = 0.0
        self._initialised = False

    def load(self) -> bool:
        if not self.projects_dir.exists():
            return False

        now_ts = datetime.now(timezone.utc).timestamp()
        cutoff = now_ts - self.RETENTION_DAYS * 86400

        files: list[str] = []
        max_mtime = 0.0
        for p in self.projects_dir.rglob("*.jsonl"):
            try:
                mtime = p.stat().st_mtime
            except OSError:
                continue
            max_mtime = max(max_mtime, mtime)
            if mtime >= cutoff:
                files.append(str(p).replace("\\", "/"))

        if max_mtime <= self._last_max_mtime and self._initialised:
            return False
        if not files:
            self._last_max_mtime = max_mtime
            return False

        file_list = "[" + ", ".join(f"'{f}'" for f in files) + "]"
        self._con.execute("DROP TABLE IF EXISTS entries")
        try:
            self._con.execute(
                f"""
                CREATE TEMP TABLE entries AS
                SELECT ts, src, input_tokens, output_tokens,
                       cache_creation_input_tokens, cache_read_input_tokens
                FROM (
                    SELECT
                        try_cast(timestamp AS TIMESTAMP) AS ts,
                        filename                         AS src,
                        COALESCE(message.usage.input_tokens, 0)::BIGINT AS input_tokens,
                        COALESCE(message.usage.output_tokens, 0)::BIGINT AS output_tokens,
                        COALESCE(message.usage.cache_creation_input_tokens, 0)::BIGINT
                            AS cache_creation_input_tokens,
                        COALESCE(message.usage.cache_read_input_tokens, 0)::BIGINT
                            AS cache_read_input_tokens,
                        ROW_NUMBER() OVER (
                            PARTITION BY COALESCE(message.id, gen_random_uuid()::TEXT)
                            ORDER BY try_cast(timestamp AS TIMESTAMP) DESC
                        ) AS rn,
                    FROM read_json_auto({file_list},
                        format='newline_delimited',
                        union_by_name=true,
                        ignore_errors=true,
                        filename=true)
                    WHERE try_cast(timestamp AS TIMESTAMP) IS NOT NULL
                ) WHERE rn = 1
                """
            )
            self._last_max_mtime = max_mtime
            self._initialised = True
            n = self._con.execute("SELECT COUNT(*) FROM entries").fetchone()[0]
            log.info("Loaded %d files, %d rows", len(files), n)
            return True
        except Exception as exc:
            log.error("Failed to load: %s", exc)
            return False

    def query_since(self, since_utc: datetime) -> dict:
        since_str = since_utc.strftime("%Y-%m-%d %H:%M:%S")
        try:
            row = self._con.execute(
                """
                SELECT
                    COALESCE(SUM(input_tokens), 0)::BIGINT,
                    COALESCE(SUM(output_tokens), 0)::BIGINT,
                    COALESCE(SUM(cache_creation_input_tokens), 0)::BIGINT,
                    COALESCE(SUM(cache_read_input_tokens), 0)::BIGINT
                FROM entries
                WHERE ts >= ?::TIMESTAMP
                """,
                [since_str],
            ).fetchone()
            if row is None:
                return _empty_tokens()
            return {
                "input_tokens": row[0], "output_tokens": row[1],
                "cache_creation_input_tokens": row[2], "cache_read_input_tokens": row[3],
            }
        except Exception as exc:
            log.warning("Query failed: %s", exc)
            return _empty_tokens()

    def query_cost_timeline(self, since_utc: datetime) -> list[tuple[datetime, float]]:
        """Return (timestamp, cost) pairs for charting."""
        since_str = since_utc.strftime("%Y-%m-%d %H:%M:%S")
        try:
            rows = self._con.execute(
                """
                SELECT ts,
                    (input_tokens / 1e6) * ? +
                    (output_tokens / 1e6) * ? +
                    (cache_creation_input_tokens / 1e6) * ? +
                    (cache_read_input_tokens / 1e6) * ?
                FROM entries
                WHERE ts >= ?::TIMESTAMP
                ORDER BY ts
                """,
                [PRICING["input"], PRICING["output"],
                 PRICING["cache_write"], PRICING["cache_read"], since_str],
            ).fetchall()
            return [(r[0], r[1]) for r in rows]
        except Exception as exc:
            log.warning("Cost timeline query failed: %s", exc)
            return []

    def query_cost_breakdown(self, since_utc: datetime) -> dict:
        """Return per-category dollar costs."""
        since_str = since_utc.strftime("%Y-%m-%d %H:%M:%S")
        try:
            row = self._con.execute(
                """
                SELECT
                    COALESCE(SUM(input_tokens), 0) / 1e6 * ?,
                    COALESCE(SUM(output_tokens), 0) / 1e6 * ?,
                    COALESCE(SUM(cache_creation_input_tokens), 0) / 1e6 * ?,
                    COALESCE(SUM(cache_read_input_tokens), 0) / 1e6 * ?
                FROM entries
                WHERE ts >= ?::TIMESTAMP
                """,
                [PRICING["input"], PRICING["output"],
                 PRICING["cache_write"], PRICING["cache_read"], since_str],
            ).fetchone()
            if row is None:
                return {"input": 0, "output": 0, "cache_write": 0, "cache_read": 0}
            return {
                "input": row[0], "output": row[1],
                "cache_write": row[2], "cache_read": row[3],
            }
        except Exception as exc:
            log.warning("Cost breakdown query failed: %s", exc)
            return {"input": 0, "output": 0, "cache_write": 0, "cache_read": 0}

    def query_token_timeline(self, since_utc: datetime) -> list[tuple]:
        """Return (ts, input, output, cache_write, cache_read) for charting."""
        since_str = since_utc.strftime("%Y-%m-%d %H:%M:%S")
        try:
            rows = self._con.execute(
                """
                SELECT ts, input_tokens, output_tokens,
                       cache_creation_input_tokens, cache_read_input_tokens
                FROM entries
                WHERE ts >= ?::TIMESTAMP
                ORDER BY ts
                """,
                [since_str],
            ).fetchall()
            return rows
        except Exception as exc:
            log.warning("Token timeline query failed: %s", exc)
            return []

    def count_sessions(self, since_utc: datetime) -> tuple[int, int]:
        """Return (today, active) from already-loaded DuckDB data."""
        since_str = since_utc.strftime("%Y-%m-%d %H:%M:%S")
        active_str = (datetime.now(timezone.utc) - timedelta(minutes=30)).strftime(
            "%Y-%m-%d %H:%M:%S"
        )
        try:
            row = self._con.execute(
                """
                SELECT
                    COUNT(DISTINCT src) FILTER (WHERE ts >= ?::TIMESTAMP),
                    COUNT(DISTINCT src) FILTER (WHERE ts >= ?::TIMESTAMP)
                FROM entries
                """,
                [since_str, active_str],
            ).fetchone()
            return (row[0], row[1]) if row else (0, 0)
        except Exception as exc:
            log.warning("Session count query failed: %s", exc)
            return (0, 0)


def _empty_tokens():
    return {
        "input_tokens": 0, "output_tokens": 0,
        "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0,
    }


def estimate_cost(t: dict) -> float:
    c = 0.0
    c += (t["input_tokens"] / 1_000_000) * PRICING["input"]
    c += (t["output_tokens"] / 1_000_000) * PRICING["output"]
    c += (t["cache_creation_input_tokens"] / 1_000_000) * PRICING["cache_write"]
    c += (t["cache_read_input_tokens"] / 1_000_000) * PRICING["cache_read"]
    return c


def fmt_tokens(n: int) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}K"
    return str(n)


def bucket_cost_timeline(points: list[tuple[datetime, float]], since: datetime, until: datetime, n_buckets: int = 32) -> list[float]:
    total_seconds = (until - since).total_seconds() or 1
    buckets = [0.0] * n_buckets
    for ts, cost in points:
        if not isinstance(ts, datetime):
            continue
        ts_local = ts.astimezone(TZ) if ts.tzinfo else ts.replace(tzinfo=TZ)
        offset = (ts_local - since).total_seconds()
        idx = int((offset / total_seconds) * n_buckets)
        idx = max(0, min(n_buckets - 1, idx))
        buckets[idx] += cost
    return buckets


def bucket_token_timeline(
    points: list[tuple], since: datetime, until: datetime, n_buckets: int = 32,
) -> list[tuple[int, int, int, int]]:
    total_seconds = (until - since).total_seconds() or 1
    buckets = [(0, 0, 0, 0)] * n_buckets
    for row in points:
        ts = row[0]
        if ts is None:
            continue
        if not isinstance(ts, datetime):
            continue
        ts_local = ts.astimezone(TZ) if ts.tzinfo else ts.replace(tzinfo=TZ)
        offset = (ts_local - since).total_seconds()
        idx = int((offset / total_seconds) * n_buckets)
        idx = max(0, min(n_buckets - 1, idx))
        inp, out, cw, cr = buckets[idx]
        buckets[idx] = (inp + row[1], out + row[2], cw + row[3], cr + row[4])
    return buckets


def day_start() -> datetime:
    """Return today's DAY_START_HOUR in configured timezone. If before that hour, use yesterday's."""
    now_local = datetime.now(TZ)
    start = now_local.replace(hour=DAY_START_HOUR, minute=0, second=0, microsecond=0)
    if now_local < start:
        start -= timedelta(days=1)
    return start


# ── GUI ───────────────────────────────────────────────────────────────
N_BUCKETS = 48
CHART_H = 60
TIME_PAD = 28

class LocDock(tk.Tk):
    def __init__(self):
        super().__init__()

        self.title("LOC Dock")
        self.overrideredirect(True)
        self.attributes("-topmost", True)
        self.attributes("-alpha", 0.92)
        self.configure(bg=BG)

        self.store = UsageStore(PROJECTS_DIR)
        self._git_points: list = []
        self._cost_points: list[tuple[datetime, float]] = []
        self._token_points: list[tuple] = []
        self._cost_breakdown: dict = {"input": 0, "output": 0, "cache_write": 0, "cache_read": 0}
        self._time_start_str = "07:00"
        self._time_end_str = "now"
        self._time_start_dt = day_start()
        self._time_end_dt = datetime.now(TZ)
        self._sess_today = 0
        self._sess_active = 0
        self._chart_mode = "loc"
        self._chart_modes = ["loc", "cost", "tokens"]
        self._tooltip: tk.Toplevel | None = None
        self._data_loaded = False
        self._spinner_frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
        self._spinner_idx = 0
        self._spinner_job = None

        root = tk.Frame(self, bg=BG, padx=4, pady=3)
        root.pack(fill="both", expand=True)

        # ── Top row: LOC | cost | sessions | close ──
        top = tk.Frame(root, bg=BG)
        top.pack(fill="x")

        self.lbl_added = tk.Label(
            top, text="+0", bg=BG, fg=GREEN, font=("Segoe UI", 8, "bold"),
        )
        self.lbl_added.pack(side="left")
        self.lbl_deleted = tk.Label(
            top, text="-0", bg=BG, fg=RED, font=("Segoe UI", 8, "bold"),
        )
        self.lbl_deleted.pack(side="left", padx=(4, 0))

        tk.Frame(top, bg=DIM, width=1, height=12).pack(side="left", padx=6)

        self.lbl_cost = tk.Label(
            top, text="$0", bg=BG, fg=ACCENT2, font=("Segoe UI", 8, "bold"),
        )
        self.lbl_cost.pack(side="left")

        self.lbl_info = tk.Label(
            top, text="i", bg=BG, fg=DIM,
            font=("Segoe UI", 7, "italic"), cursor="hand2",
        )
        self.lbl_info.pack(side="left", padx=(2, 0))
        self.lbl_info.bind("<Enter>", self._show_tooltip)
        self.lbl_info.bind("<Leave>", self._hide_tooltip)

        tk.Frame(top, bg=DIM, width=1, height=12).pack(side="left", padx=6)

        tk.Label(
            top, text="S:", bg=BG, fg=DIM, font=("Segoe UI", 7),
        ).pack(side="left")
        self.lbl_sess_active = tk.Label(
            top, text="0", bg=BG, fg=ORANGE, font=("Segoe UI", 7, "bold"),
        )
        self.lbl_sess_active.pack(side="left", padx=(2, 0))
        tk.Label(
            top, text="/", bg=BG, fg=DIM, font=("Segoe UI", 7),
        ).pack(side="left", padx=(1, 0))
        self.lbl_sess_today = tk.Label(
            top, text="0", bg=BG, fg=FG, font=("Segoe UI", 7),
        )
        self.lbl_sess_today.pack(side="left", padx=(1, 0))

        close_btn = tk.Label(
            top, text="x", bg=BG, fg=DIM,
            font=("Segoe UI", 8), cursor="hand2",
        )
        close_btn.pack(side="right")
        close_btn.bind("<Button-1>", lambda e: self.destroy())

        self.lbl_spinner = tk.Label(
            top, text="", bg=BG, fg=DIM, font=("Segoe UI", 7),
        )
        self.lbl_spinner.pack(side="right", padx=(0, 3))

        # ── Chart ──
        self.chart = tk.Canvas(
            root, height=CHART_H, width=1,
            bg=CHART_BG, highlightthickness=0, bd=0,
        )
        self.chart.pack(fill="x", expand=True, pady=(2, 0))
        self.chart.bind("<Configure>", lambda e: self._draw_chart())
        self.chart.bind("<Button-1>", self._toggle_chart)

        # ── Bottom row: token breakdown ──
        bot = tk.Frame(root, bg=BG)
        bot.pack(fill="x", pady=(2, 0))

        self.lbl_input = self._inline_stat(bot, "IN", FG)
        self.lbl_output = self._inline_stat(bot, "OUT", PINK)
        self.lbl_cache_w = self._inline_stat(bot, "CW", YELLOW)
        self.lbl_cache_r = self._inline_stat(bot, "CR", BLUE)
        self.lbl_total = self._inline_stat(bot, "TOT")

        # ── Drag ──
        self._drag_x = 0
        self._drag_y = 0
        for w in [self, root, top]:
            w.bind("<Button-1>", self._start_drag)
            w.bind("<B1-Motion>", self._on_drag)

        # ── Position bottom-right, above taskbar ──
        self.update_idletasks()
        sw = self.winfo_screenwidth()
        sh = self.winfo_screenheight()
        w = self.winfo_reqwidth()
        h = self.winfo_reqheight()
        taskbar_h = sh - self.winfo_vrootheight()
        if taskbar_h < 20:
            taskbar_h = 48
        self.geometry(f"+{sw - w}+{sh - h - taskbar_h}")

        self.after(100, self._load_data)

    def _inline_stat(self, parent, label, color=FG):
        tk.Label(
            parent, text=label, bg=BG, fg=DIM, font=("Segoe UI", 7),
        ).pack(side="left", padx=(3, 0))
        val = tk.Label(
            parent, text="--", bg=BG, fg=color, font=("Segoe UI", 7),
        )
        val.pack(side="left")
        return val

    def _toggle_chart(self, event=None):
        idx = self._chart_modes.index(self._chart_mode)
        self._chart_mode = self._chart_modes[(idx + 1) % len(self._chart_modes)]
        self._draw_chart()

    def _show_tooltip(self, event=None):
        self._hide_tooltip()
        cb = self._cost_breakdown
        total = sum(cb.values())
        text = (
            f"IN   ${cb['input']:.2f}  @$15/MTok\n"
            f"OUT  ${cb['output']:.2f}  @$75/MTok\n"
            f"CW   ${cb['cache_write']:.2f}  @$18.75/MTok\n"
            f"CR   ${cb['cache_read']:.2f}  @$1.50/MTok\n"
            f"─────────────────\n"
            f"TOT  ${total:.2f}"
        )
        tw = tk.Toplevel(self)
        tw.overrideredirect(True)
        tw.attributes("-topmost", True)
        tw.configure(bg="#222244")
        lbl = tk.Label(
            tw, text=text, bg="#222244", fg=FG,
            font=("Consolas", 8), justify="left", padx=6, pady=4,
        )
        lbl.pack()
        x = self.lbl_info.winfo_rootx()
        y = self.lbl_info.winfo_rooty() + self.lbl_info.winfo_height() + 2
        tw.geometry(f"+{x}+{y}")
        self._tooltip = tw

    def _hide_tooltip(self, event=None):
        if self._tooltip:
            self._tooltip.destroy()
            self._tooltip = None

    def _start_drag(self, event):
        self._drag_x = event.x
        self._drag_y = event.y

    def _on_drag(self, event):
        self.geometry(
            f"+{self.winfo_x() + event.x - self._drag_x}"
            f"+{self.winfo_y() + event.y - self._drag_y}"
        )

    def _draw_chart(self):
        c = self.chart
        c.delete("all")
        w = c.winfo_width()
        h = CHART_H
        if w < 10:
            return

        if not self._data_loaded:
            c.create_rectangle(0, 0, w, h, fill=CHART_BG, outline="")
            frame = self._spinner_frames[self._spinner_idx]
            c.create_text(
                w // 2, h // 2, text=f"{frame} loading…",
                fill=DIM, font=("Segoe UI", 8),
            )
            return

        if self._chart_mode == "loc":
            self._draw_loc_chart(c, w, h)
        elif self._chart_mode == "cost":
            self._draw_cost_chart(c, w, h)
        elif self._chart_mode == "tokens":
            self._draw_token_chart(c, w, h)

    def _draw_time_labels(self, c, w, axis_y):
        since = self._time_start_dt
        now = self._time_end_dt
        span = (now - since).total_seconds()
        if span <= 0:
            return
        font = ("Segoe UI", 7)
        tick_h = 3
        c.create_text(4, axis_y + 2, text=self._time_start_str,
                       fill=DIM, font=font, anchor="nw")
        first_tick = since.replace(minute=0, second=0, microsecond=0)
        if first_tick <= since:
            first_tick += timedelta(hours=3 - (first_tick.hour % 3) if first_tick.hour % 3 else 3)
        t = first_tick
        while t < now:
            frac = (t - since).total_seconds() / span
            x = TIME_PAD + frac * (w - 2 * TIME_PAD)
            if x > TIME_PAD + 12:
                c.create_line(x, axis_y, x, axis_y + tick_h, fill=AXIS_CLR)
                c.create_text(x, axis_y + 3, text=t.strftime("%H"),
                              fill=DIM, font=font, anchor="n")
            t += timedelta(hours=3)

    def _draw_chrome(self, c, w, y_max_str: str):
        c.create_text(
            w - 3, 2, text=self._chart_mode.upper(),
            fill=DIM, font=("Segoe UI", 6), anchor="ne",
        )
        if y_max_str:
            c.create_text(
                3, 2, text=y_max_str,
                fill=DIM, font=("Segoe UI", 6), anchor="nw",
            )

    def _draw_loc_chart(self, c, w, h):
        buckets = bucket_timeline(self._git_points, day_start(), datetime.now(TZ), N_BUCKETS)
        bottom = h - 18
        c.create_line(0, bottom, w, bottom, fill=AXIS_CLR, width=1)
        self._draw_time_labels(c, w, bottom)

        if not buckets or all(a == 0 and d == 0 for a, d in buckets):
            self._draw_chrome(c, w, "")
            c.create_text(w // 2, h // 2, text="no commits yet",
                          fill=DIM, font=("Segoe UI", 8))
            return

        bar_left = TIME_PAD
        bar_zone = w - 2 * TIME_PAD
        if bar_zone < 10:
            return

        max_val = max(a + d for a, d in buckets) or 1
        self._draw_chrome(c, w, f"{max_val:,}")
        bar_w = max(bar_zone / len(buckets), 1)
        usable_h = bottom - 4

        for i, (a, d) in enumerate(buckets):
            if a == 0 and d == 0:
                continue
            x0 = bar_left + i * bar_w
            x1 = x0 + bar_w - 1
            total = a + d
            total_h = max((total / max_val) * usable_h, 1)
            green_h = (a / total) * total_h if total else 0
            red_h = total_h - green_h
            y = bottom
            if green_h > 0:
                c.create_rectangle(x0, y - green_h, x1, y, fill=GREEN, outline="")
                y -= green_h
            if red_h > 0:
                c.create_rectangle(x0, y - red_h, x1, y, fill=RED, outline="")

    def _draw_cost_chart(self, c, w, h):
        since = day_start()
        buckets = bucket_cost_timeline(self._cost_points, since, datetime.now(TZ), N_BUCKETS)
        bottom = h - 18
        c.create_line(0, bottom, w, bottom, fill=AXIS_CLR, width=1)
        self._draw_time_labels(c, w, bottom)

        if not buckets or all(v == 0 for v in buckets):
            self._draw_chrome(c, w, "")
            c.create_text(w // 2, h // 2, text="no cost data",
                          fill=DIM, font=("Segoe UI", 8))
            return

        bar_left = TIME_PAD
        bar_zone = w - 2 * TIME_PAD
        if bar_zone < 10:
            return
        max_val = max(buckets) or 1
        self._draw_chrome(c, w, f"${max_val:.2f}")
        bar_w = max(bar_zone / len(buckets), 1)
        usable_h = bottom - 4

        for i, v in enumerate(buckets):
            if v <= 0:
                continue
            x0 = bar_left + i * bar_w
            x1 = x0 + bar_w - 1
            bh = max((v / max_val) * usable_h, 1)
            c.create_rectangle(x0, bottom - bh, x1, bottom, fill=ACCENT2, outline="")

    def _draw_token_chart(self, c, w, h):
        since = day_start()
        buckets = bucket_token_timeline(self._token_points, since, datetime.now(TZ), N_BUCKETS)
        bottom = h - 18
        c.create_line(0, bottom, w, bottom, fill=AXIS_CLR, width=1)
        self._draw_time_labels(c, w, bottom)

        if not buckets or all(sum(b) == 0 for b in buckets):
            self._draw_chrome(c, w, "")
            c.create_text(w // 2, h // 2, text="no token data",
                          fill=DIM, font=("Segoe UI", 8))
            return

        bar_left = TIME_PAD
        bar_zone = w - 2 * TIME_PAD
        if bar_zone < 10:
            return

        max_val = max(sum(b) for b in buckets) or 1
        self._draw_chrome(c, w, fmt_tokens(max_val))
        bar_w = max(bar_zone / len(buckets), 1)
        usable_h = bottom - 4

        for i, (inp, out, cw, cr) in enumerate(buckets):
            x0 = bar_left + i * bar_w
            x1 = x0 + bar_w - 1
            y = bottom
            for val, color in zip([cr, cw, out, inp], [BLUE, YELLOW, PINK, FG]):
                if val <= 0:
                    continue
                bh = max((val / max_val) * usable_h, 1)
                c.create_rectangle(x0, y - bh, x1, y, fill=color, outline="")
                y -= bh

    def _start_spinner(self):
        self._spinner_idx = (self._spinner_idx + 1) % len(self._spinner_frames)
        if self._data_loaded:
            self.lbl_spinner.config(text=self._spinner_frames[self._spinner_idx])
        else:
            self._draw_chart()
        self._spinner_job = self.after(80, self._start_spinner)

    def _stop_spinner(self):
        if self._spinner_job:
            self.after_cancel(self._spinner_job)
            self._spinner_job = None
        self.lbl_spinner.config(text="")

    def _load_data(self):
        if not self._data_loaded:
            self._draw_chart()
        self._start_spinner()
        threading.Thread(target=self._load_data_bg, daemon=True).start()

    def _load_data_bg(self):
        """Heavy I/O: git subprocesses, JSONL filesystem scan. Runs every 60s."""
        since = day_start()

        try:
            self._git_points = get_git_loc_timeline(REPOS_DIR, since)
        except Exception as exc:
            log.warning("Git failed: %s", exc)

        try:
            self.store.load()
        except Exception as exc:
            log.error("Store load: %s", exc)

        since_utc = since.astimezone(timezone.utc)

        try:
            self._cost_points = self.store.query_cost_timeline(since_utc)
        except Exception as exc:
            log.warning("Cost timeline failed: %s", exc)

        try:
            self._token_points = self.store.query_token_timeline(since_utc)
        except Exception as exc:
            log.warning("Token timeline failed: %s", exc)

        try:
            self._cost_breakdown = self.store.query_cost_breakdown(since_utc)
        except Exception as exc:
            log.warning("Cost breakdown failed: %s", exc)

        try:
            self._sess_today, self._sess_active = self.store.count_sessions(since_utc)
        except Exception as exc:
            log.warning("Session count failed: %s", exc)

        self.after(0, self._on_load_done)

    def _on_load_done(self):
        self._data_loaded = True
        self._stop_spinner()
        self._update_ui()
        self.after(REFRESH_DATA_MS, self._load_data)

    def _update_ui(self):
        """Cheap: query cached DuckDB, rebucket, update labels. Runs every 30s."""
        since = day_start()
        now_local = datetime.now(TZ)

        self._time_start_str = since.strftime("%H:%M")
        self._time_end_str = now_local.strftime("%H:%M")
        self._time_start_dt = since
        self._time_end_dt = now_local
        self._draw_chart()

        loc_buckets = bucket_timeline(self._git_points, since, now_local, N_BUCKETS)
        total_added = sum(a for a, _ in loc_buckets)
        total_deleted = sum(d for _, d in loc_buckets)
        self.lbl_added.config(text=f"+{total_added:,}")
        self.lbl_deleted.config(text=f"-{total_deleted:,}")

        since_utc = since.astimezone(timezone.utc)
        t = self.store.query_since(since_utc)
        total = (t["input_tokens"] + t["output_tokens"]
                 + t["cache_creation_input_tokens"] + t["cache_read_input_tokens"])

        self.lbl_input.config(text=fmt_tokens(t["input_tokens"]))
        self.lbl_output.config(text=fmt_tokens(t["output_tokens"]))
        self.lbl_cache_w.config(text=fmt_tokens(t["cache_creation_input_tokens"]))
        self.lbl_cache_r.config(text=fmt_tokens(t["cache_read_input_tokens"]))
        self.lbl_total.config(text=fmt_tokens(total))
        self.lbl_cost.config(text=f"${estimate_cost(t):.2f}")

        self.lbl_sess_active.config(text=str(self._sess_active))
        self.lbl_sess_today.config(text=str(self._sess_today))

        self.after(REFRESH_UI_MS, self._update_ui)


if __name__ == "__main__":
    log.info("LOC Dock starting")
    try:
        app = LocDock()
        app.mainloop()
    except KeyboardInterrupt:
        log.info("Shutdown")
    except Exception as exc:
        log.exception("Fatal: %s", exc)
        raise
