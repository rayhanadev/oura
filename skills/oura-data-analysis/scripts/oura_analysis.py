#!/usr/bin/env python3
"""Read-only health snapshots for the local Oura Turso/SQLite database."""

from __future__ import annotations

import argparse
import json
import math
import sqlite3
import statistics
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import quote


DEFAULT_DB = Path.home() / ".oura" / "oura.db"
HEALTH_EVENTS = {
    "green_ibi_quality_event",
    "ibi_and_amplitude_event",
    "hrv_event",
    "temp_event",
    "temp_period",
    "sleep_temp_event",
    "motion_event",
    "motion_period",
    "activity_information",
    "spo2_event",
    "spo2_r_pi_event",
    "sleep_phase_information",
    "sleep_phase_details",
    "sleep_phase_data",
}


@dataclass(frozen=True)
class Anchor:
    tick: int
    unix: int
    captured: int


@dataclass
class DayMetrics:
    heart_rates: list[float] = field(default_factory=list)
    ring_resting_heart_rates: list[float] = field(default_factory=list)
    ibis: int = 0
    good_ibis: int = 0
    ibi_diff_squares: list[float] = field(default_factory=list)
    ring_rmssd: list[float] = field(default_factory=list)
    temperatures: list[float] = field(default_factory=list)
    sleep_temperatures: list[float] = field(default_factory=list)
    ecore_temperatures: list[float] = field(default_factory=list)
    motion_seconds: int = 0
    motion_records: int = 0
    mets: list[float] = field(default_factory=list)
    spo2: list[float] = field(default_factory=list)
    spo2_signal_samples: int = 0
    sleep_phases: dict[str, list[str]] = field(default_factory=lambda: defaultdict(list))
    decoded_records: int = 0
    night_signal_start_unix: int | None = None
    night_signal_end_unix: int | None = None


NIGHT_EVENTS = {
    "hrv_event",
    "sleep_temp_event",
    "spo2_event",
    "spo2_r_pi_event",
    "sleep_phase_information",
    "sleep_phase_details",
    "sleep_phase_data",
}


def fail(message: str) -> None:
    raise SystemExit(f"error: {message}")


def open_database(path: Path) -> sqlite3.Connection:
    path = path.expanduser().resolve()
    if not path.is_file():
        fail(f"database not found: {path}")
    uri = f"file:{quote(str(path), safe='/')}?mode=ro"
    try:
        connection = sqlite3.connect(uri, uri=True)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA query_only = ON")
        tables = {
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table'"
            )
        }
    except sqlite3.Error as error:
        fail(f"cannot open {path}: {error}")
    required = {"events", "readings", "sync_runs", "schema_migrations"}
    if missing := required - tables:
        fail(f"not an Oura database; missing tables: {', '.join(sorted(missing))}")
    return connection


def decode(text: str | None) -> dict[str, Any] | None:
    if not text:
        return None
    try:
        value = json.loads(text)
    except (TypeError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def load_anchors(connection: sqlite3.Connection) -> list[Anchor]:
    anchors = []
    rows = connection.execute(
        "SELECT ring_timestamp, captured_unix, decoded_json "
        "FROM events WHERE name = 'time_sync' AND decoded_json IS NOT NULL"
    )
    for row in rows:
        value = decode(row["decoded_json"])
        unix = value.get("unix_time") if value else None
        if isinstance(unix, int):
            anchors.append(Anchor(row["ring_timestamp"], unix, row["captured_unix"]))
    return anchors


def estimate_unix(row: sqlite3.Row, anchors: list[Anchor]) -> int | None:
    if not anchors:
        return None
    tick = int(row["ring_timestamp"])
    captured = int(row["captured_unix"])
    anchor = min(
        anchors,
        key=lambda item: (abs(item.captured - captured), abs(item.tick - tick)),
    )
    return round(anchor.unix + (tick - anchor.tick) / 10)


def local_iso(timestamp: int | None) -> str | None:
    if timestamp is None:
        return None
    try:
        return datetime.fromtimestamp(timestamp).astimezone().isoformat(timespec="seconds")
    except (OSError, OverflowError, ValueError):
        return None


def median(values: Iterable[float]) -> float | None:
    collected = list(values)
    return round(statistics.median(collected), 2) if collected else None


def mean(values: Iterable[float]) -> float | None:
    collected = list(values)
    return round(statistics.fmean(collected), 2) if collected else None


def numeric_list(value: dict[str, Any], key: str) -> list[float]:
    values = value.get(key)
    if not isinstance(values, list):
        return []
    return [float(item) for item in values if isinstance(item, (int, float))]


def health_rows(connection: sqlite3.Connection) -> Iterable[sqlite3.Row]:
    placeholders = ",".join("?" for _ in HEALTH_EVENTS)
    return connection.execute(
        f"SELECT name, ring_timestamp, captured_unix, decoded_json FROM events "
        f"WHERE decoded_json IS NOT NULL AND name IN ({placeholders}) "
        "ORDER BY captured_unix, rowid",
        sorted(HEALTH_EVENTS),
    )


def collect_days(
    connection: sqlite3.Connection, anchors: list[Anchor], days: int
) -> dict[date, DayMetrics]:
    first_day = date.today() - timedelta(days=days - 1)
    output: dict[date, DayMetrics] = defaultdict(DayMetrics)
    for row in health_rows(connection):
        timestamp = estimate_unix(row, anchors)
        if timestamp is None:
            continue
        try:
            event_day = datetime.fromtimestamp(timestamp).astimezone().date()
        except (OSError, OverflowError, ValueError):
            continue
        if not first_day <= event_day <= date.today():
            continue
        value = decode(row["decoded_json"])
        if not value or value.get("_status") == "unvalidated":
            continue
        metrics = output[event_day]
        metrics.decoded_records += 1
        name = row["name"]

        heart_rates = [
            sample for sample in numeric_list(value, "hr_bpm") if 25 <= sample <= 240
        ]
        metrics.heart_rates.extend(heart_rates)
        if name == "hrv_event":
            metrics.ring_resting_heart_rates.extend(heart_rates)
        if name in NIGHT_EVENTS:
            metrics.night_signal_start_unix = (
                timestamp
                if metrics.night_signal_start_unix is None
                else min(metrics.night_signal_start_unix, timestamp)
            )
            metrics.night_signal_end_unix = (
                timestamp
                if metrics.night_signal_end_unix is None
                else max(metrics.night_signal_end_unix, timestamp)
            )

        ibis = numeric_list(value, "ibi_ms")
        qualities = numeric_list(value, "quality")
        if qualities and len(qualities) == len(ibis):
            metrics.ibis += len(ibis)
            metrics.good_ibis += sum(quality == 1 for quality in qualities)
            for index in range(1, len(ibis)):
                if qualities[index - 1] == qualities[index] == 1:
                    difference = ibis[index] - ibis[index - 1]
                    metrics.ibi_diff_squares.append(difference * difference)

        metrics.ring_rmssd.extend(
            sample for sample in numeric_list(value, "rmssd_ms") if 0 < sample <= 500
        )
        if name in {"temp_event", "temp_period", "sleep_temp_event"}:
            temperatures = [
                sample for sample in numeric_list(value, "temps_c") if 20 <= sample <= 45
            ]
            metrics.temperatures.extend(temperatures)
            if name == "sleep_temp_event":
                metrics.sleep_temperatures.extend(temperatures)
            elif name == "temp_event":
                metrics.ecore_temperatures.extend(temperatures)
        if name in {"motion_event", "motion_period"}:
            metrics.motion_records += 1
            motion_seconds = value.get("motion_seconds")
            if isinstance(motion_seconds, int) and motion_seconds >= 0:
                metrics.motion_seconds += motion_seconds
        if name == "activity_information":
            metrics.mets.extend(
                sample for sample in numeric_list(value, "met") if 0 <= sample <= 50
            )
        if name == "spo2_event":
            metrics.spo2.extend(
                sample for sample in numeric_list(value, "spo2_percent") if 50 <= sample <= 100
            )
        if name == "spo2_r_pi_event":
            metrics.spo2_signal_samples += len(numeric_list(value, "r"))
        phases = value.get("phases")
        if isinstance(phases, list):
            metrics.sleep_phases[name].extend(
                phase for phase in phases if phase in {"deep", "light", "rem", "awake"}
            )
    return output


def nightly_temperature(samples: list[float]) -> float | None:
    """Port open_oura's recovered ecore nightly-temperature filter."""
    centi = [round(sample * 100) for sample in samples if sample > 0]
    ring = [0] * 7
    ring_index = 0
    window_min = 2**16 - 1
    window_max = 0
    maxima = []
    for index, sample in enumerate(centi):
        ring[ring_index] = sample
        ring_index = 0 if ring_index == 6 else ring_index + 1
        filtered = sorted(ring)[3]
        if filtered:
            window_min = min(window_min, filtered)
            window_max = max(window_max, filtered)
        if (index + 1) % 30 == 0:
            if window_max and window_max >= window_min and window_max - window_min < 250:
                maxima.append(window_max)
            window_min = 2**16 - 1
            window_max = 0
    return round(min(maxima) / 100, 2) if len(maxima) >= 4 else None


def sleep_summary(metrics: DayMetrics) -> dict[str, Any]:
    phases = []
    source = None
    for event_name in (
        "sleep_phase_data",
        "sleep_phase_details",
        "sleep_phase_information",
    ):
        if metrics.sleep_phases.get(event_name):
            source = event_name
            phases = metrics.sleep_phases[event_name]
            break
    counts = Counter(phases)
    asleep = counts["deep"] + counts["light"] + counts["rem"]
    total = len(phases)
    wake_count = 0
    found_sleep = False
    previous = "awake"
    for phase in phases:
        if phase != "awake":
            found_sleep = True
        if found_sleep and phase == "awake" and previous != "awake":
            wake_count += 1
        previous = phase
    return {
        "source_event": source,
        "epoch_seconds": 30,
        "time_in_bed_minutes": round(total / 2, 1) if phases else None,
        "total_sleep_minutes": round(asleep / 2, 1) if phases else None,
        "deep_minutes": round(counts["deep"] / 2, 1) if phases else None,
        "light_minutes": round(counts["light"] / 2, 1) if phases else None,
        "rem_minutes": round(counts["rem"] / 2, 1) if phases else None,
        "awake_minutes": round(counts["awake"] / 2, 1) if phases else None,
        "efficiency_percent": round(asleep / total * 100, 1) if total else None,
        "wake_transitions": wake_count if phases else None,
        "stage_epochs": total,
    }


def serialize_day(day: date, metrics: DayMetrics) -> dict[str, Any]:
    rmssd_estimate = (
        round(math.sqrt(statistics.fmean(metrics.ibi_diff_squares)), 2)
        if metrics.ibi_diff_squares
        else None
    )
    quality = (
        round(metrics.good_ibis / metrics.ibis * 100, 1) if metrics.ibis else None
    )
    nightly_temperature_value = median(metrics.sleep_temperatures)
    nightly_temperature_source = (
        "sleep_temp_event_median" if nightly_temperature_value is not None else None
    )
    ecore_temperature = nightly_temperature(metrics.ecore_temperatures)
    if nightly_temperature_value is None and ecore_temperature is not None:
        nightly_temperature_value = ecore_temperature
        nightly_temperature_source = "recovered_ecore_filter"
    night_window_seconds = (
        metrics.night_signal_end_unix - metrics.night_signal_start_unix
        if metrics.night_signal_start_unix is not None
        and metrics.night_signal_end_unix is not None
        else None
    )
    return {
        "date": day.isoformat(),
        "heart_rate_median_bpm": median(metrics.heart_rates),
        "heart_rate_samples": len(metrics.heart_rates),
        "resting_heart_rate_median_bpm": median(metrics.ring_resting_heart_rates),
        "resting_heart_rate_lowest_bpm": (
            min(metrics.ring_resting_heart_rates)
            if metrics.ring_resting_heart_rates
            else None
        ),
        "resting_heart_rate_samples": len(metrics.ring_resting_heart_rates),
        "ibi_rmssd_estimate_ms": rmssd_estimate,
        "ring_hrv_rmssd_median_ms": median(metrics.ring_rmssd),
        "ring_hrv_rmssd_samples": len(metrics.ring_rmssd),
        "ibi_good_quality_percent": quality,
        "ibi_samples": metrics.ibis,
        "temperature_skin_median_c": median(metrics.temperatures),
        "nightly_temperature_estimate_c": nightly_temperature_value,
        "nightly_temperature_source": nightly_temperature_source,
        "ecore_temperature_estimate_c": ecore_temperature,
        "temperature_baseline_14d_c": None,
        "temperature_deviation_c": None,
        "temperature_samples": len(metrics.temperatures),
        "night_signal_window": {
            "start": local_iso(metrics.night_signal_start_unix),
            "end": local_iso(metrics.night_signal_end_unix),
            "hours": (
                round(night_window_seconds / 3600, 2)
                if night_window_seconds is not None
                else None
            ),
            "estimated": True,
        },
        "motion_detected_seconds": metrics.motion_seconds,
        "motion_records": metrics.motion_records,
        "activity_met_median": median(metrics.mets),
        "activity_met_mean": mean(metrics.mets),
        "activity_met_samples": len(metrics.mets),
        "spo2_median_percent": median(metrics.spo2),
        "spo2_percent_samples": len(metrics.spo2),
        "spo2_uncalibrated_signal_samples": metrics.spo2_signal_samples,
        "sleep": sleep_summary(metrics),
        "decoded_health_records": metrics.decoded_records,
    }


def add_temperature_baselines(rows: list[dict[str, Any]]) -> None:
    for index, row in enumerate(rows):
        current = row["nightly_temperature_estimate_c"]
        prior = [
            candidate["nightly_temperature_estimate_c"]
            for candidate in rows[max(0, index - 14) : index]
            if candidate["nightly_temperature_estimate_c"] is not None
        ]
        if current is not None and len(prior) >= 7:
            baseline = median(prior)
            row["temperature_baseline_14d_c"] = baseline
            row["temperature_deviation_c"] = round(current - baseline, 2)


def command_overview(connection: sqlite3.Connection, db: Path) -> dict[str, Any]:
    anchors = load_anchors(connection)
    counts = {
        row["name"]: row["count"]
        for row in connection.execute(
            "SELECT name, COUNT(*) AS count FROM events GROUP BY name ORDER BY count DESC"
        )
    }
    bounds = [None, None]
    for row in connection.execute(
        "SELECT ring_timestamp, captured_unix FROM events ORDER BY captured_unix, rowid"
    ):
        if timestamp := estimate_unix(row, anchors):
            bounds[0] = timestamp if bounds[0] is None else min(bounds[0], timestamp)
            bounds[1] = timestamp if bounds[1] is None else max(bounds[1], timestamp)
    battery = connection.execute(
        "SELECT value, captured_unix FROM readings WHERE kind = 'battery_percent' "
        "ORDER BY captured_unix DESC LIMIT 1"
    ).fetchone()
    run = connection.execute(
        "SELECT status, started_unix, finished_unix, inserted, error FROM sync_runs "
        "ORDER BY started_unix DESC LIMIT 1"
    ).fetchone()
    version = connection.execute(
        "SELECT MAX(version) FROM schema_migrations"
    ).fetchone()[0]
    return {
        "database": str(db.expanduser().resolve()),
        "schema_version": version,
        "event_records": sum(counts.values()),
        "decoded_records": connection.execute(
            "SELECT COUNT(*) FROM events WHERE decoded_json IS NOT NULL"
        ).fetchone()[0],
        "time_sync_anchors": len(anchors),
        "estimated_event_range": {
            "first": local_iso(bounds[0]),
            "last": local_iso(bounds[1]),
            "warning": "estimated from ring ticks and time_sync anchors",
        },
        "latest_battery": (
            {
                "percent": round(float(battery["value"]), 1),
                "captured_at": local_iso(battery["captured_unix"]),
            }
            if battery
            else None
        ),
        "last_sync": (
            {
                "status": run["status"],
                "finished_at": local_iso(run["finished_unix"] or run["started_unix"]),
                "inserted": run["inserted"],
                "error": run["error"],
            }
            if run
            else None
        ),
        "health_event_counts": {
            name: counts[name] for name in sorted(HEALTH_EVENTS) if name in counts
        },
        "limitations": [
            "event dates are estimates, not stored measurement timestamps",
            "decoded_json is best-effort reverse-engineered output",
            "official Oura scores are not present",
        ],
    }


def command_daily(
    connection: sqlite3.Connection, days: int
) -> dict[str, Any]:
    anchors = load_anchors(connection)
    by_day = collect_days(connection, anchors, days)
    first_day = date.today() - timedelta(days=days - 1)
    rows = [
        serialize_day(day, by_day.get(day, DayMetrics()))
        for day in (first_day + timedelta(days=offset) for offset in range(days))
    ]
    add_temperature_baselines(rows)
    return {
        "range": {"from": first_day.isoformat(), "through": date.today().isoformat()},
        "timezone": datetime.now().astimezone().tzname(),
        "event_dates_are_estimated": True,
        "derivations": {
            "heart_rate": "median of decoder-accepted plausible BPM samples",
            "resting_heart_rate": "median and minimum of ring-provided five-minute nighttime HR summaries",
            "ibi_rmssd": "sqrt(mean(square(diff(IBI)))) over adjacent quality-accepted daytime IBI pairs",
            "ring_hrv": "median of nonzero ring-provided five-minute nighttime RMSSD summaries",
            "nightly_temperature": "median of dedicated sleep-temperature samples, with recovered ecore filtering only as fallback",
            "temperature_deviation": "nightly estimate minus median of up to 14 prior valid nights; requires seven prior nights",
            "sleep": "30-second ring sleep-stage epochs; uses the most detailed available sleep-phase event",
            "activity": "summary statistics over decoded MET samples; cadence and calories are not inferred",
        },
        "days": rows,
    }


def personal_comparison(
    current: float | None,
    prior_rows: list[dict[str, Any]],
    field_name: str,
    minimum_days: int = 7,
) -> dict[str, Any]:
    values = [
        row[field_name] for row in prior_rows if row.get(field_name) is not None
    ]
    baseline = median(values[-60:]) if len(values) >= minimum_days else None
    return {
        "value": current,
        "personal_baseline": baseline,
        "change_from_baseline": (
            round(current - baseline, 2)
            if current is not None and baseline is not None
            else None
        ),
        "baseline_days": len(values),
        "calibrating": len(values) < minimum_days,
    }


def command_snapshot(connection: sqlite3.Connection) -> dict[str, Any]:
    rows = command_daily(connection, 61)["days"]
    populated = [row for row in rows if row["decoded_health_records"]]
    current = populated[-1] if populated else rows[-1]
    prior_rows = [row for row in populated if row["date"] < current["date"]]
    sleep = current["sleep"]

    resting_hr = personal_comparison(
        current["resting_heart_rate_median_bpm"],
        prior_rows,
        "resting_heart_rate_median_bpm",
    )
    resting_hr["lowest_bpm"] = current["resting_heart_rate_lowest_bpm"]
    hrv = personal_comparison(
        current["ring_hrv_rmssd_median_ms"],
        prior_rows,
        "ring_hrv_rmssd_median_ms",
    )
    temperature = personal_comparison(
        current["nightly_temperature_estimate_c"],
        prior_rows,
        "nightly_temperature_estimate_c",
    )
    temperature["source"] = current["nightly_temperature_source"]

    missing = []
    if not prior_rows:
        missing.append("personal recovery baseline")
    if sleep["total_sleep_minutes"] is None:
        missing.append("sleep stages and duration")
    if current["spo2_median_percent"] is None:
        missing.append("calibrated blood-oxygen percentage")

    return {
        "date": current["date"],
        "snapshot_type": "early_snapshot" if len(prior_rows) < 7 else "personal_trend",
        "readiness": {
            "score": None,
            "label": "Calibrating" if len(prior_rows) < 7 else "Personal trend available",
            "resting_heart_rate_bpm": resting_hr,
            "hrv_rmssd_ms": hrv,
            "nighttime_skin_temperature_c": temperature,
        },
        "sleep": {
            "score": None,
            "label": "Measured" if sleep["total_sleep_minutes"] is not None else "Stages unavailable",
            "total_sleep_minutes": sleep["total_sleep_minutes"],
            "time_in_bed_minutes": sleep["time_in_bed_minutes"],
            "efficiency_percent": sleep["efficiency_percent"],
            "deep_minutes": sleep["deep_minutes"],
            "rem_minutes": sleep["rem_minutes"],
            "restfulness": (
                {"wake_transitions": sleep["wake_transitions"]}
                if sleep["wake_transitions"] is not None
                else None
            ),
            "nighttime_sensor_window": current["night_signal_window"],
        },
        "activity": {
            "score": None,
            "label": "In progress" if current["date"] == date.today().isoformat() else "Partial",
            "movement_minutes": round(current["motion_detected_seconds"] / 60),
            "average_met": current["activity_met_mean"],
            "median_met": current["activity_met_median"],
        },
        "breathing": {
            "label": "Available" if current["spo2_median_percent"] is not None else "Not available",
            "average_spo2_percent": current["spo2_median_percent"],
        },
        "confidence": {
            "usable_prior_days": len(prior_rows),
            "baseline_status": "calibrating" if len(prior_rows) < 7 else "available",
            "missing": missing,
            "notes": [
                "Do not infer readiness direction until a personal baseline exists."
                if len(prior_rows) < 7
                else "Compare current contributors with the local personal baseline.",
                "The nighttime sensor window is coverage, not sleep duration."
                if sleep["total_sleep_minutes"] is None
                else "Sleep duration comes from decoded 30-second stage epochs.",
                "Scores are intentionally omitted because official Oura combiners are unavailable.",
            ],
        },
    }


COMPARE_FIELDS = [
    "heart_rate_median_bpm",
    "resting_heart_rate_median_bpm",
    "ring_hrv_rmssd_median_ms",
    "temperature_skin_median_c",
    "nightly_temperature_estimate_c",
    "temperature_deviation_c",
    "activity_met_median",
    "spo2_median_percent",
]


def command_compare(
    connection: sqlite3.Connection, recent: int, baseline: int
) -> dict[str, Any]:
    total_days = recent + baseline
    daily = command_daily(connection, total_days)["days"]
    older, newer = daily[:baseline], daily[baseline:]
    comparisons = {}
    for field_name in COMPARE_FIELDS:
        baseline_values = [row[field_name] for row in older if row[field_name] is not None]
        recent_values = [row[field_name] for row in newer if row[field_name] is not None]
        baseline_median = median(baseline_values)
        recent_median = median(recent_values)
        comparisons[field_name] = {
            "baseline_median": baseline_median,
            "recent_median": recent_median,
            "absolute_change": (
                round(recent_median - baseline_median, 2)
                if baseline_median is not None and recent_median is not None
                else None
            ),
            "baseline_days_with_data": len(baseline_values),
            "recent_days_with_data": len(recent_values),
            "enough_data_for_trend": len(baseline_values) >= 7
            and len(recent_values) >= 3,
        }
    return {
        "baseline_window_days": baseline,
        "recent_window_days": recent,
        "comparison": comparisons,
        "warning": "changes are descriptive, not diagnostic; inspect coverage before interpreting",
    }


def command_events(
    connection: sqlite3.Connection, days: int, name: str | None, samples: int
) -> dict[str, Any]:
    anchors = load_anchors(connection)
    earliest = datetime.now().astimezone() - timedelta(days=days)
    parameters: tuple[Any, ...] = (name,) if name else ()
    where = "WHERE name = ?" if name else ""
    aggregates: dict[str, dict[str, Any]] = {}
    examples: list[dict[str, Any]] = []
    for row in connection.execute(
        f"SELECT name, ring_timestamp, captured_unix, decoded_json FROM events {where} "
        "ORDER BY captured_unix DESC, rowid DESC",
        parameters,
    ):
        timestamp = estimate_unix(row, anchors)
        if timestamp is None:
            continue
        measured = datetime.fromtimestamp(timestamp).astimezone()
        if measured < earliest:
            continue
        event_name = row["name"]
        entry = aggregates.setdefault(
            event_name,
            {"records": 0, "decoded": 0, "first_estimated": None, "last_estimated": None},
        )
        entry["records"] += 1
        value = decode(row["decoded_json"])
        entry["decoded"] += value is not None
        measured_iso = measured.isoformat(timespec="seconds")
        entry["first_estimated"] = (
            measured_iso
            if entry["first_estimated"] is None
            else min(entry["first_estimated"], measured_iso)
        )
        entry["last_estimated"] = (
            measured_iso
            if entry["last_estimated"] is None
            else max(entry["last_estimated"], measured_iso)
        )
        if name and len(examples) < samples:
            examples.append(
                {
                    "estimated_at": measured_iso,
                    "decoded": value,
                }
            )
    return {
        "window_days": days,
        "event_dates_are_estimated": True,
        "events": dict(sorted(aggregates.items())),
        "samples": examples,
    }


def positive(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Summarize a local Oura database without modifying it. Output is JSON."
    )
    result.add_argument("--db", type=Path, default=DEFAULT_DB, help="database path")
    subcommands = result.add_subparsers(dest="command", required=True)
    subcommands.add_parser("snapshot", help="show a health-first daily snapshot")
    subcommands.add_parser("overview", help="show freshness, coverage, and provenance")
    daily = subcommands.add_parser("daily", help="show one row of measurements per day")
    daily.add_argument("--days", type=positive, default=14)
    compare = subcommands.add_parser("compare", help="compare a recent window to an earlier baseline")
    compare.add_argument("--recent", type=positive, default=7)
    compare.add_argument("--baseline", type=positive, default=28)
    events = subcommands.add_parser("events", help="show event and decoder coverage")
    events.add_argument("--days", type=positive, default=30)
    events.add_argument("--name", help="limit to one exact event name")
    events.add_argument("--samples", type=positive, default=3)
    return result


def main() -> None:
    args = parser().parse_args()
    connection = open_database(args.db)
    try:
        if args.command == "snapshot":
            result = command_snapshot(connection)
        elif args.command == "overview":
            result = command_overview(connection, args.db)
        elif args.command == "daily":
            result = command_daily(connection, args.days)
        elif args.command == "compare":
            result = command_compare(connection, args.recent, args.baseline)
        elif args.command == "events":
            result = command_events(connection, args.days, args.name, args.samples)
        else:
            fail(f"unknown command: {args.command}")
        print(json.dumps(result, indent=2, sort_keys=False, allow_nan=False))
    except (sqlite3.Error, ValueError) as error:
        fail(str(error))
    finally:
        connection.close()


if __name__ == "__main__":
    main()
