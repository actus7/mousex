## What does this change?

<!-- A short description, and why it is needed. Link related issues: "Closes #123". -->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes (on macOS, add `--exclude mousex-native`)
- [ ] `cargo test --workspace` passes (on macOS, add `--exclude mousex-native`)
- [ ] Editor checks pass if `app/` changed: `npm ci && npm test && npm run build`
- [ ] New logic has tests
- [ ] Documentation is updated where needed (README, README.pt-BR, docs/ARCHITECTURE.md, CHANGELOG under `Unreleased`)
- [ ] No secrets, personal data, or private logs in the diff or in the description
- [ ] The commit messages follow Conventional Commits

## Tested on

<!-- Which operating systems and versions did you test on? Which role (main or controlled computer)? -->

- [ ] Windows (version: )
- [ ] macOS (version: )
- [ ] Not tested on real hardware (explain why)

## How was it tested?

<!-- Steps you took, including the two-computer scenario when it applies. -->

## Notes for reviewers

<!-- Anything that needs special attention: risks, trade-offs, or areas you are unsure about. -->
