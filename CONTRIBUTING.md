# Contributing to Email by YCorp

Thanks for helping out! Bug reports, fixes, docs and design feedback are all
welcome.

## License of contributions

Email by YCorp is licensed under the [MIT License](LICENSE). By submitting a
contribution, you agree to license it under the same MIT terms.

Only contribute work that you have the right to license under MIT. Third-party
code or assets need a compatible license and an entry in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Before you start

- Ask questions and propose ideas in [Discussions](https://github.com/YCorpLabs/Email/discussions). Open an
  issue for a concrete bug.
- For larger changes, describe the plan first so we can agree on the approach.
- Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Development setup

Build from source as described in the [README](README.md#build-from-source).
Plain builds use the neutral `unofficial` [distribution profile](docs/distribution-profile.md).
They never touch an official installation's data. Never commit
`config/*.local*`, OAuth client values, signing material or real mail. They
belong in ignored local files or your environment.

Use a test mailbox, or an isolated cache via `EMAIL_STORE_PATH`, while
developing. Do not paste real messages into issues, tests or fixtures.

## Checks

Run what applies to your change and say in the pull request what you ran:

```sh
cargo test --workspace --locked              # engine, MCP and profile parsers
python3 scripts/check-release-swift.py       # macOS rule harnesses
python3 scripts/distribution-profile.test.py
python3 scripts/validate-local.py --profile source --gitleaks /path/to/gitleaks   # Gitleaks 8.30.1
```

The Linux shell builds and tests in the container described in
[apps/linux/README.md](apps/linux/README.md).

## Pull requests

- Keep each pull request focused, with a clear description of what changed and
  why.
- **Both desktop shells:** macOS (`apps/macos`) and Linux (`apps/linux`) are
  separate native apps with the same design. A change to shared UI behavior,
  such as the reader, composer, navigation, command palette or settings, must
  update both, or state the remaining gap plainly. Include screenshots of both
  for visible changes.
- Add or update tests with behavior changes.
- Update documentation when behavior or setup changes.
- Never weaken credential storage, signature checks, HTML sanitization or MCP
  permission boundaries to make something work.

## Forks

See [FORKING.md](FORKING.md) for shipping your own build and
[TRADEMARKS.md](TRADEMARKS.md) for the name and logo.
