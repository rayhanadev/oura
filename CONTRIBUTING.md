# Contributing

Thanks for helping improve oura-cli.

## Before opening a change

- Search existing issues and pull requests.
- Keep ring auth keys, databases, BLE captures, serials, MAC addresses, and
  health data out of issues and commits.
- Discuss protocol changes with upstream `open_oura` when they belong in the
  transport or decoder rather than this sync application.
- Preserve raw event bodies and transactional cursor semantics.
- Gate every state-changing ring operation behind explicit user intent.

## Development

Use a recent stable Rust toolchain. On Linux, install BlueZ and D-Bus development
packages first.

Run all local checks:

~~~bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s tests
python3 skills/oura-data-analysis/scripts/oura_analysis.py --help
~~~

Do not use a real personal database in tests. Build fixtures in a temporary
directory and use `--home` or `--db` to isolate manual tests.

## Pull requests

Explain the user-visible behavior, compatibility impact, test coverage, and any
new privacy or ring-state risk. Keep commits focused. Documentation and CLI help
must change with the behavior they describe.
