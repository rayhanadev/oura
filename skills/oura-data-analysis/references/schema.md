# Local Oura database schema

The CLI stores an embedded Turso database at `~/.oura/oura.db` by default. Turso's local file is SQLite-compatible. The bundled analysis script opens it with SQLite read-only and query-only modes.

## Tables

### `events`

One losslessly retained ring history event per row.

| Column | Meaning |
| --- | --- |
| `event_key` | Stable BLAKE3 deduplication key |
| `serial` | Ring serial; sensitive identifier |
| `tag` | Numeric BLE event tag |
| `name` | Stable decoder event name |
| `ring_timestamp` | Ring tick counter in deciseconds; not Unix time |
| `body` | Original event body; sensitive opaque bytes |
| `decoded_json` | Best-effort structured decode, or null |
| `decoder_revision` | Pinned `open_oura` decoder commit |
| `captured_unix` | Host time when the sync stored the event |

Common decoded fields:

| Event | Fields | Interpretation |
| --- | --- | --- |
| `green_ibi_quality_event` | `ibi_ms`, `quality`, `hr_bpm` | Daytime green-PPG inter-beat intervals; `hr_bpm` contains decoder-accepted beats |
| `ibi_and_amplitude_event` | `ibi_ms`, `amplitude`, `hr_bpm` | Usually overnight IBI and optical amplitude |
| `hrv_event` | `hr_bpm`, `rmssd_ms`, `interval_min` | Ring-provided five-minute HR/RMSSD summaries |
| `temp_event`, `temp_period`, `sleep_temp_event` | `temps_c` | Absolute finger skin-sensor samples |
| `motion_event` | `motion_seconds`, axes, intensity, orientation | Compact accelerometer summary |
| `motion_period` | `motion_levels` | Packed relative motion levels |
| `activity_information` | `met`, `state` | Ring-computed MET samples; sample interval is not exposed here |
| `spo2_event` | `spo2_percent` | Direct percentage samples when emitted |
| `spo2_r_pi_event` | `r`, `perfusion_index` | Raw optical ratio and perfusion signal; not SpO2 percentages |
| `sleep_phase_*` | `phases` | Ring sleep-stage codes when available |
| `time_sync` | `unix_time` | Anchor mapping a ring tick to Unix time |

Unknown and unvalidated decodes retain their raw bodies. A decoded JSON object containing `"_status":"unvalidated"` is not suitable for health interpretation.

### `readings`

Host-polled point readings. Currently includes `battery_percent`; `captured_unix` is an actual Unix timestamp.

### `devices`

Cached identity and firmware data. Serial and MAC address are sensitive and are suppressed by the analysis script.

### `sync_state`

The next ring cursor and last committed sync time per ring.

### `sync_runs`

Each foreground or background sync attempt, including cursor range, received/inserted counts, status, and errors.

## Event time estimation

Ring event timestamps are decisecond ticks. A `time_sync` event supplies a pair `(ring_tick, unix_time)`, so estimated event time is:

```text
unix_time + (event_ring_tick - anchor_ring_tick) / 10
```

The analysis script chooses the time-sync anchor captured closest to the event, then the closest tick. This is usually suitable within one ring boot epoch. A factory reset, clock adjustment, tick reset/wrap, or sparse time-sync history can make historical dates approximate. Always label these dates as estimated and use `captured_unix` only as ingestion time, not measurement time.

## Provenance levels

1. `body`: lossless bytes directly fetched from the ring.
2. `decoded_json`: best-effort `open_oura` decoder output pinned by `decoder_revision`.
3. Script aggregates: medians, counts, RMSSD estimates, and comparisons computed locally from decoded JSON.
4. Agent interpretation: an inference that must preserve the limitations of levels 1–3.
