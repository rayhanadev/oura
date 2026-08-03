# Presenting a local Oura health snapshot

## Default structure

Mirror the questions answered by Oura's main views without claiming its scores:

1. **Bottom line**: State whether the result is a personal trend or still calibrating.
2. **Recovery**: Resting heart rate, HRV, nighttime temperature deviation, and their direction versus personal baselines.
3. **Sleep**: Total sleep, efficiency, timing, deep/REM sleep, and restfulness when decoded.
4. **Activity**: Meaningful progress or balance for the current day; avoid judging an incomplete day.
5. **Breathing**: Report average SpO2 only when a calibrated percentage exists.

Use `Calibrating`, `In progress`, or `Unavailable` as honest states. Do not manufacture a score or replace missing contributors with zero.

## What to hide by default

Do not surface database paths, schema versions, serials, MAC addresses, sync cursors, inserted rows, decoder names, raw event counts, or uncalibrated optical-signal counts in a health summary. These belong in diagnostics, not the health snapshot.

Do not list every metric. Choose the few values that answer “How did I recover?”, “How did I sleep?”, and “How is today going?” Explain a missing contributor only when it prevents an answer.

## Calibration and comparisons

Recovery signals are personal. Oura's Readiness contributors compare current and recent measurements with learned personal averages; several contributors use recent 14-day data and longer-term averages. Use local baselines only when coverage is adequate and identify them as local comparisons.

With fewer than seven prior usable nights:

- label the result `Calibrating`;
- report measured resting HR, HRV, and nighttime skin temperature without rating them;
- avoid generic “good,” “bad,” or “normal” judgments;
- tell the user that trend interpretation will improve with consistent wear.

## Language

Prefer: “Your overnight HRV was 53 ms; there is not yet a personal baseline to say whether that is high or low for you.”

Avoid: “HRV: 53 ms from 57 samples; database has 14,972 events.”

When sleep phases are missing but nighttime sensing exists, say that the ring captured overnight recovery signals but local sleep staging is unavailable. A sensor-coverage window is not sleep duration.

## Official concept references

- Oura Readiness Contributors: https://support.ouraring.com/hc/en-us/articles/360057791533-Readiness-Contributors
- Oura Sleep Contributors: https://support.ouraring.com/hc/en-us/articles/360057792293-Sleep-Contributors
- Oura Activity Contributors: https://support.ouraring.com/hc/en-us/articles/360055901214-Activity-Contributors
- Oura Blood Oxygen Sensing: https://support.ouraring.com/hc/en-us/articles/7328398760851-Blood-Oxygen-Sensing-SpO2
