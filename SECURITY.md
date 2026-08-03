# Security policy

## Sensitive data

An Oura database contains private health measurements and device identifiers.
The ring key in `~/.oura/secrets/ring.key` authenticates to the physical ring.
Never attach either file, raw BLE captures, serial numbers, or MAC addresses to
a public report.

## Reporting a vulnerability

Use GitHub's private security-advisory feature for vulnerabilities involving key
exposure, unsafe ring commands, file permissions, command injection, database
corruption, or unintended health-data disclosure. Include a minimal reproduction
that uses synthetic data. Do not open a public issue until a fix is available.

## Supported versions

Until the first stable release, only the latest commit on the default branch is
supported. Security fixes will be documented in release notes.
