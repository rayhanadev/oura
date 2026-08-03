# oura-cli

Private, local-first Oura Ring sync over Bluetooth.

`oura` copies the ring's lossless history stream into an embedded
[Turso database](https://github.com/tursodatabase/turso) on your computer. It
does not require Oura Cloud, keeps the raw event bodies for future decoders, and
runs as a short periodic user job instead of holding a permanent Bluetooth
connection.

This is an unofficial project built on the reverse-engineered
[`open_oura`](https://github.com/Th0rgal/open_oura) protocol. It is not affiliated
with or endorsed by Oura Health Oy.

## What it does

- Incrementally syncs ring history with an atomic, resumable cursor.
- Stores raw events and best-effort decoded JSON in a private local Turso file.
- Shows a compact daily overview without connecting to the ring.
- Caches ring identity, firmware, battery, and sync health.
- Sends one desktop notification when a sync sees battery below 10%.
- Runs every 30 minutes with launchd on macOS or a systemd user timer on Linux.
- Ships an agent skill for local trend, RMSSD, temperature, sleep, movement, MET,
  and SpO2 analysis.

The default installation is per-user and needs no `sudo`:

~~~text
~/.oura/
  oura.db              local Turso database
  config.toml          ring and path configuration
  secrets/ring.key     16-byte BLE app-auth key
  logs/                background output on macOS
  run/                 single-sync lock

~/.local/bin/oura      installed executable
~~~

## Compatibility

macOS is the primary platform. This project has been exercised with Oura Ring 4
and includes the Bluetooth privacy metadata and launchd integration needed for a
background command-line tool.

Linux support is experimental but complete enough to try: it uses BlueZ through
`open_oura`, a systemd user timer, the system journal, and `notify-send` for
battery alerts. Gen 3, Ring 4, and newer rings share the protocol surface in
`open_oura`, but this client has only been verified here with Ring 4. Windows is
not currently supported.

## Install from source

Requirements:

- Rust with Cargo and a compiler toolchain
- macOS, or Linux with BlueZ, systemd, D-Bus development headers, and `pkg-config`
- Python 3 only if you want to use the bundled analysis skill

On Debian or Ubuntu, the native packages are:

~~~bash
sudo apt install bluez libdbus-1-dev libnotify-bin pkg-config
~~~

Build and install:

~~~bash
cargo test
cargo build --release
./target/release/oura install
~~~

The installer creates the database and configuration, copies only the executable
to `~/.local/bin/oura`, and starts a 30-minute user schedule. It does not copy a
runtime or another nested binary into `~/.oura`.

If `~/.local/bin` is not already on your PATH, the installer prints the exact
line to add to your shell profile:

~~~bash
export PATH="$HOME/.local/bin:$PATH"
~~~

Choose another interval that divides evenly into an hour:

~~~bash
./target/release/oura install --interval 15
~~~

## Pairing and auth

Ring history is protected by a 16-byte app-auth key. A key file is one line with
32 hexadecimal characters.

If you already have the key for the ring:

~~~bash
oura configure --key-file /secure/path/ring.key
oura doctor --connect
oura sync
~~~

For an intentionally factory-reset ring:

~~~bash
oura pair --confirm-reset
~~~

Pairing creates and privately stores a key, sets the ring clock, and enables
automatic daytime heart-rate and SpO2 measurements. It is the only normal command
that installs a key or changes measurement features.

Important: a factory reset clears the ring's prior app authentication. Pairing it
with this client does not also reconnect it to the official app. Returning to the
official app may require another reset and onboarding. Normal sync never sends a
factory-reset, DFU, RData, or feature-change command.

If more than one ring is nearby:

~~~bash
oura scan
oura configure --address DEVICE_ID
~~~

## Everyday use

~~~bash
oura status
oura sync
oura info
~~~

`status` and `info` use the local cache and return immediately. Use
`oura info --refresh` only for a live Bluetooth refresh, and add `--verbose` for
low-level hardware fields.

Useful diagnostics:

~~~bash
oura doctor
oura doctor --connect
oura logs
~~~

Ring advertisements are intermittent. The default discovery window is 60
seconds:

~~~bash
oura configure --scan-timeout 90
~~~

## Background sync

~~~bash
oura service stop       # fully disable the schedule
oura service start      # enable it and sync now
oura service restart
oura service status     # raw launchd or systemd state
~~~

When the ring is away or the computer is asleep, that run is skipped. A later
run resumes from the last committed cursor. Event rows and the cursor update are
committed in one database transaction.

### Repeated Bluetooth prompts on macOS

To stop prompts immediately, disable the background schedule:

~~~bash
oura service stop
~~~

Then open System Settings > Privacy & Security > Bluetooth and allow the
installed Oura CLI executable. Run `oura doctor --connect` once interactively,
then use `oura service start`.

Source builds are ad-hoc signed by default. macOS identifies an ad-hoc signature
as one specific build, so replacing the executable during development can require
Bluetooth approval again. A developer with a stable code-signing identity can
use it during installation:

~~~bash
OURA_CODESIGN_IDENTITY="Apple Development: Your Name" \
  ./target/release/oura install
~~~

Published macOS binaries should be signed with a stable Developer ID identity.

## Low-battery notifications

Every successful ring connection caches the battery level. When it first drops
strictly below 10%, the sync sends one desktop notification. It does not repeat
while the battery remains low; reaching 10% or higher rearms it.

~~~bash
oura notifications status
oura notifications test
oura configure --battery-threshold 15
oura configure --battery-threshold 0    # disable
~~~

macOS uses Notification Center through `osascript`. Linux uses `notify-send`.
Failure to display a notification is logged and never fails the data sync.

## Analyze the data with an agent

The repository includes the standard agent skill
[`skills/oura-data-analysis`](skills/oura-data-analysis/SKILL.md). It keeps
analysis local and calculates supported metrics from decoded ring signals:

- quality-filtered heart-rate and IBI coverage;
- RMSSD over adjacent good-quality IBI samples;
- the recovered ecore nightly skin-temperature filter and rolling deviation;
- sleep-stage durations and efficiency when phase events exist;
- movement and MET summaries;
- direct SpO2 summaries when percentage samples exist;
- recent-versus-personal-baseline comparisons.

The deterministic tool can also be run directly:

~~~bash
python3 skills/oura-data-analysis/scripts/oura_analysis.py snapshot
python3 skills/oura-data-analysis/scripts/oura_analysis.py overview
python3 skills/oura-data-analysis/scripts/oura_analysis.py daily --days 14
python3 skills/oura-data-analysis/scripts/oura_analysis.py compare \
  --recent 7 --baseline 28
~~~

`snapshot` organizes the useful measurements into readiness, sleep, activity,
and breathing domains while keeping diagnostics out of the health summary. The
tool opens the database read-only and emits JSON. To make the skill discoverable
by Codex, copy or link it into your personal skills directory:

~~~bash
mkdir -p "$HOME/.codex/skills"
cp -R skills/oura-data-analysis "$HOME/.codex/skills/"
~~~

The local calculations are not automatically identical to Oura's official
Readiness, Sleep, Activity, or Stress scores. `open_oura` has recovered many
component algorithms, but some exact score tables, model files, user-profile
inputs, and historical baseline state are unavailable. The skill reports
provenance and missing inputs instead of inventing parity.

Oura Ring is not a medical device. This project and its analysis skill are for
personal information and research, not diagnosis or treatment.

## Data model and backups

The Turso file is SQLite-compatible and contains:

- `events`: raw bodies, decoded JSON, ring ticks, and decoder revision;
- `readings`: host-polled values such as battery percentage;
- `devices`: cached ring identity and firmware;
- `sync_state`: the next ring cursor;
- `sync_runs`: audit history for each sync;
- `notification_state`: low-battery deduplication state.

Back up the complete `~/.oura` directory while no sync is running. Keep backups
encrypted: the database contains health data and device identifiers, and
`secrets/ring.key` can authenticate to the ring.

The process uses a `077` umask. State directories are owner-only; the database,
configuration, key, logs, and sidecars are owner-readable only. The auth key is
never written to the database or normal logs. FileVault or full-disk encryption
is strongly recommended.

## Uninstall

~~~bash
oura uninstall
~~~

This removes the background service and `~/.local/bin/oura`. It deliberately
preserves `~/.oura`. Remove that directory separately only when you intend to
delete the database, configuration, logs, and ring key.

## Development

~~~bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s tests
~~~

See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance and
[SECURITY.md](SECURITY.md) for reporting security issues.

## Acknowledgements

This client depends on [`open_oura`](https://github.com/Th0rgal/open_oura) for
the BLE transport and event decoders, and on
[Turso](https://github.com/tursodatabase/turso) for the embedded database.

## License

MIT. See [LICENSE](LICENSE).
