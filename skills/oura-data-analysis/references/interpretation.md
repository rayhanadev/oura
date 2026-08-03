# Interpreting local Oura signals

## Principles

- Analyze coverage before values. Motion, cold fingers, fit, charging, non-wear, firmware behavior, and disabled features can all create gaps.
- Prefer multiple-day within-person changes over a single reading or generic threshold.
- Separate measurement time from sync time. Ring event dates are estimated from `time_sync` anchors.
- Keep official app metrics separate from local event aggregates. Official scores and baselines use versioned algorithms and additional context that this project does not promise to reproduce.

## Heart rate and IBI

`hrv_event` contains the ring's five-minute nighttime heart-rate and RMSSD summaries. Prefer its nonzero values for a recovery snapshot. `green_ibi_quality_event.hr_bpm` is calculated from decoder-accepted green-PPG inter-beat intervals and is useful for daytime detail.

Oura's daytime heart-rate feature normally samples for one minute every five minutes and may skip periods with motion or poor temperature/signal conditions. Missing samples are therefore not zero heart rate or proof of non-wear.

`ibi_rmssd_estimate_ms` is RMSSD over adjacent, explicitly quality-accepted IBI pairs within retained daytime event packets. Do not calculate RMSSD from `ibi_and_amplitude_event` because it has no equivalent per-beat quality flags and can contain large optical outliers. Packet boundaries, daytime sampling, decoder filtering, and gaps make the local estimate different from Oura's nighttime HRV.

Official references:

- Oura, Heart Rate Graph: https://support.ouraring.com/hc/en-us/articles/4410656562579-Heart-Rate-Graph
- Oura, Resting Heart Rate: https://support.ouraring.com/hc/en-us/articles/360025588793-Resting-Heart-Rate
- Oura, Respiratory Rate (IBI background): https://support.ouraring.com/hc/en-us/articles/360025443174-Respiratory-Rate

## Temperature

The local decoder exposes absolute finger skin-sensor samples. These are not core, basal, or oral temperature. Prefer the dedicated `sleep_temp_event` median for the nightly snapshot. The recovered ecore filter over `temp_event` is a fallback when dedicated sleep samples are absent. Oura's app expresses nighttime temperature relative to a personal baseline built over time, so the absolute value alone should not be rated.

Official reference:

- Oura, Body Temperature: https://support.ouraring.com/hc/en-us/articles/360025587493-Body-Temperature

## Movement and MET

Movement events and MET samples are compact, ring-computed summaries from the accelerometer. MET expresses energy expenditure relative to rest, but this local schema does not expose enough stable cadence/profile context to claim calories, steps, active minutes, workout classification, or an Activity Score. Use counts and medians as coverage/activity proxies only.

Official references:

- Oura, How Oura Measures Steps & Activity: https://support.ouraring.com/hc/en-us/articles/360025576833-How-Oura-Measures-Steps-Activity
- Oura glossary: https://support.ouraring.com/hc/en-us/articles/5949130374547-Glossary

## SpO2

Oura measures peripheral oxygen saturation during detected sleep, usually for sleep periods longer than three hours. Fit, motion, hand position, altitude, congestion, circulation, and battery can affect coverage and values. `spo2_percent` is a decoded percentage sample. `r` and `perfusion_index` from `spo2_r_pi_event` are uncalibrated optical inputs and must not be described as saturation percentages.

Official reference:

- Oura, Blood Oxygen Sensing: https://support.ouraring.com/hc/en-us/articles/7328398760851-Blood-Oxygen-Sensing-SpO2

## Locally recoverable calculations

The bundled tool calculates directly from decoded event data:

- nighttime resting HR and RMSSD from ring-provided five-minute summaries;
- diagnostic RMSSD from adjacent, explicitly quality-accepted daytime IBI pairs;
- dedicated nighttime skin-temperature medians, the recovered ecore filter as fallback, and a transparent rolling personal baseline;
- 30-second sleep-stage duration, efficiency, and wake-transition summaries when sleep-phase events exist;
- movement duration and MET sample summaries;
- direct SpO2 percentage summaries when calibrated percentage events exist.

These calculations can be meaningful without official scores. For composite scores, the pinned `open_oura` algorithm status says some exact contributor mappings, weights, and runtime tables remain unresolved. Proprietary sleep/activity model files are not distributed. A locally defined composite can still be calculated on request, but its formula must be explicit and it must not be called the official Oura score.

## Decoder and algorithm boundary

`open_oura` documents three data layers: history events, live values, and opt-in RData. It retains raw events and best-effort decodes recovered from Oura software and real captures. Its own documentation notes that some finished metrics require app-side algorithms or proprietary model artifacts. Do not claim exact parity with the official app.

Project references:

- `open_oura` data recovery map: https://github.com/Th0rgal/open_oura/blob/main/docs/data-recovery-map.md
- `open_oura` decoder notes: https://github.com/Th0rgal/open_oura/blob/main/docs/native-decoder.md
- `open_oura` model runners and limitations: https://github.com/Th0rgal/open_oura/blob/main/docs/model-runners.md

## Safety language

Oura states that the ring is not a medical device and is not intended to diagnose, treat, cure, monitor, or prevent medical conditions. If the user reports symptoms, repeated concerning patterns, or wants to make treatment decisions, direct them to a qualified health professional. Do not use a wearable-data analysis to reassure someone away from care.

- Oura, Medical Conditions: https://support.ouraring.com/hc/en-us/articles/360038214494-Oura-Medical-Conditions
