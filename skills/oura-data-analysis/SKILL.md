---
name: oura-data-analysis
description: Turn the private Turso/SQLite database produced by the local `oura` CLI into a concise health snapshot or personal trend analysis. Use when a user asks how they slept, recovered, or moved; asks for an Oura-style summary, recent health snapshot, heart rate, HRV, temperature, activity, SpO2, data quality, or comparisons across days; or needs a safe local query of `~/.oura/oura.db` without uploading health data.
---

# Oura Data Analysis

Turn local ring data into a health-first summary without modifying the database or sending health data over the network. Organize the answer around readiness/recovery, sleep, activity, and breathing. Treat local calculations as reverse-engineered contributors, not exact official Oura scores.

## Workflow

1. Resolve this skill's directory from the loaded `SKILL.md`. Run its script with an absolute path.
2. Start with `python3 <skill-dir>/scripts/oura_analysis.py snapshot`. Add `--db PATH` when the user selected a non-default database.
3. Use the snapshot's confidence fields internally. Do not make database health, sync counts, event names, or sample counts the user-facing summary.
4. Run another command only when the request needs it:
   - Daily measurements: `python3 <skill-dir>/scripts/oura_analysis.py daily --days 14`
   - Recent versus prior baseline: `python3 <skill-dir>/scripts/oura_analysis.py compare --recent 7 --baseline 28`
   - Sync freshness and decoder coverage: `python3 <skill-dir>/scripts/oura_analysis.py overview`
   - Event availability and decoder coverage: `python3 <skill-dir>/scripts/oura_analysis.py events --days 30`
   - Samples for one event type: add `--name EVENT_NAME --samples 5` to `events`.
5. Read [references/presentation.md](references/presentation.md) and [references/interpretation.md](references/interpretation.md) before writing a health summary. Read [references/schema.md](references/schema.md) only for custom investigation or provenance details.
6. Lead with the health interpretation and the most important context for today. Prefer within-person trends over population cutoffs.

## Calculation policy

- Calculate physiological summaries when their required raw signal and quality fields exist. Return null with the missing prerequisite when they do not.
- Prefer recovered `open_oura`/ecore formulas. The bundled script identifies each derivation in its JSON output.
- Calculate transparent personal-baseline deviations and component metrics before considering a composite score.
- Prefer ring-provided nighttime HR and RMSSD summaries for the recovery snapshot. Keep raw quality-filtered IBI RMSSD as a diagnostic calculation.
- Prefer dedicated sleep-temperature samples for the nighttime temperature contributor. Use the recovered ecore temperature filter only as a fallback.
- Only produce a composite local score when the user asks for one. Name it `local estimate`, publish its exact formula and inputs, show missing contributors, and never label it as Oura's official score.
- Do not substitute an invented formula when an exact Oura combiner is unresolved. The pinned `open_oura` research reports unresolved score weights/tables; explain this limit while still returning all calculable components.

## Guardrails

- Keep all processing local. Do not upload, paste, or transmit database contents without explicit user authorization.
- Use the bundled read-only script. Never mutate the database during analysis.
- Do not expose serial numbers, MAC addresses, auth keys, raw debug text, or raw event bodies unless the user specifically requests them.
- Call `temperature_skin_median_c` skin-sensor temperature. Do not call it core or basal body temperature.
- Call `ibi_rmssd_estimate_ms` an estimate over decoded good-quality IBI segments. Do not equate it with the official nightly HRV metric.
- Treat `spo2_r_pi_event` as an uncalibrated optical signal. Only `spo2_percent` values are percentages.
- Do not present a local Readiness, Sleep, Activity, Stress, or workout score as an official Oura result. Some combiners and model inputs remain unresolved even though many component calculations are recoverable.
- Do not diagnose disease, recommend medication changes, or present a single unusual reading as a health conclusion. If a pattern is concerning or symptoms are present, recommend discussion with a qualified clinician.
- Explain missing data as missing. Do not interpolate across gaps or silently treat missing values as zero.

## Output style

Write an Oura-style health snapshot, not a telemetry report:

1. Open with a one-sentence bottom line such as “Your ring is still calibrating, but last night's recovery signals were captured.”
2. Summarize **Recovery**, **Sleep**, **Activity**, and **Breathing** in that order. Omit an empty domain only when its absence is unimportant.
3. Explain what the values mean relative to the user's personal baseline. If fewer than seven prior usable days exist, say `Calibrating` and do not label recovery good or bad.
4. Surface only numbers that help the user understand their health: typically resting HR, HRV, temperature deviation, sleep duration/stages, activity progress, and calibrated SpO2.
5. End with one grounded takeaway. Do not prescribe behavior when the evidence cannot support it.

Never lead with sync time, battery, database rows, event names, decoder coverage, raw signal counts, or insertion counts. Mention technical coverage only when it explains a missing or uncertain health conclusion. Do not dump every available metric.
