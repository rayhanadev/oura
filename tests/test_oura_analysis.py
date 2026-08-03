import importlib.util
import json
import sqlite3
import sys
import time
import unittest
from pathlib import Path


SCRIPT = (
    Path(__file__).parents[1]
    / "skills"
    / "oura-data-analysis"
    / "scripts"
    / "oura_analysis.py"
)
SPEC = importlib.util.spec_from_file_location("oura_analysis", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class AnalysisTests(unittest.TestCase):
    def test_recovered_nightly_temperature_filter(self):
        self.assertEqual(MODULE.nightly_temperature([35.0] * 150), 35.0)
        self.assertIsNone(MODULE.nightly_temperature([35.0] * 90))

    def test_sleep_summary_uses_thirty_second_epochs(self):
        metrics = MODULE.DayMetrics()
        metrics.sleep_phases["sleep_phase_data"] = [
            "light",
            "deep",
            "rem",
            "awake",
        ]
        summary = MODULE.sleep_summary(metrics)
        self.assertEqual(summary["time_in_bed_minutes"], 2.0)
        self.assertEqual(summary["total_sleep_minutes"], 1.5)
        self.assertEqual(summary["efficiency_percent"], 75.0)
        self.assertEqual(summary["wake_transitions"], 1)

    def test_unqualified_overnight_ibi_is_not_used_for_rmssd(self):
        connection = sqlite3.connect(":memory:")
        connection.row_factory = sqlite3.Row
        connection.execute(
            "CREATE TABLE events ("
            "name TEXT, ring_timestamp INTEGER, captured_unix INTEGER, "
            "decoded_json TEXT)"
        )
        now = int(time.time())
        rows = [
            (
                "time_sync",
                1000,
                now,
                json.dumps({"unix_time": now}),
            ),
            (
                "green_ibi_quality_event",
                1001,
                now,
                json.dumps(
                    {
                        "ibi_ms": [1000, 1100, 1000],
                        "quality": [1, 1, 1],
                        "hr_bpm": [60, 55, 60],
                    }
                ),
            ),
            (
                "ibi_and_amplitude_event",
                1002,
                now,
                json.dumps({"ibi_ms": [500, 1500], "hr_bpm": [120, 40]}),
            ),
        ]
        connection.executemany("INSERT INTO events VALUES (?, ?, ?, ?)", rows)

        metrics = MODULE.collect_days(connection, MODULE.load_anchors(connection), 1)[
            MODULE.date.today()
        ]
        day = MODULE.serialize_day(MODULE.date.today(), metrics)

        self.assertEqual(day["ibi_samples"], 3)
        self.assertEqual(day["ibi_good_quality_percent"], 100.0)
        self.assertEqual(day["ibi_rmssd_estimate_ms"], 100.0)

    def test_recovery_snapshot_prefers_ring_summaries_and_sleep_temperature(self):
        metrics = MODULE.DayMetrics(
            ring_resting_heart_rates=[70, 60, 65],
            ring_rmssd=[50, 60],
            temperatures=[28.0, 35.4, 35.6],
            sleep_temperatures=[35.4, 35.6],
            ecore_temperatures=[28.0] * 150,
        )

        day = MODULE.serialize_day(MODULE.date.today(), metrics)

        self.assertEqual(day["resting_heart_rate_median_bpm"], 65)
        self.assertEqual(day["resting_heart_rate_lowest_bpm"], 60)
        self.assertEqual(day["ring_hrv_rmssd_median_ms"], 55)
        self.assertEqual(day["nightly_temperature_estimate_c"], 35.5)
        self.assertEqual(day["nightly_temperature_source"], "sleep_temp_event_median")


if __name__ == "__main__":
    unittest.main()
