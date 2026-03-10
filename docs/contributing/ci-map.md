# ZeroClaw CI/CD Map

This document maps repository paths to the GitHub Actions workflows that govern them.

For event-by-event delivery behavior across PR, merge, push, and release, see [`.github/workflows/master-branch-flow.md`](../../.github/workflows/master-branch-flow.md).

## Core Workflows

- `.github/workflows/ci.yml` (`CI`)
    - Purpose: Rust validation (`test`, `build`) on `master` PRs.
    - Behavior: Uses `cargo nextest` for testing and builds release binaries for Linux and macOS.
    - Merge gate: Required for all PRs to `master`.

- `.github/workflows/release-build.yml` (`Production Release Build`)
    - Purpose: Build reproducible production binaries on `master` pushes and `v*` tags.
    - Quality gates: `fmt`, `clippy`, and `test\? before build.

- `.github/workflows/release.yml` (`Release`)
    - Purpose: Handle GitHub release creation and artifact publishing.

## Important Workflows

- `.github/workflows/pr-check-stale.yml` (`Stale`)
    - Purpose: stale issue/PR lifecycle automation
- `.github/dependabot.yml` (`Dependabot`)
    - Purpose: grouped, rate-limited dependency update PRs (Cargo + GitHub Actions)
- `.github/workflows/pr-check-status.yml` (`PR Hygiene`)
    - Purpose: nudge stale-but-active PRs to rebase/re-run required checks before queue starvation

## Trigger Map

- `CI`: Pull requests targeting `master`.
- `Production Release Build`: Push to `master`, Push tags `v*`.
- `Release`: Tag push `v*`, Manual dispatch.
- `Security Audit`: Push to `master`, PRs to `master`, weekly schedule.

## Fast Triage Guide

1. `CI Required Gate` failing: start with `.github/workflows/ci-run.yml`.
2. Docker failures on PRs: inspect `.github/workflows/pub-docker-img.yml` `pr-smoke` job.
3. Release failures (tag/manual/scheduled): inspect `.github/workflows/pub-release.yml` and the `prepare` job outputs.
4. Homebrew formula publish failures: inspect `.github/workflows/pub-homebrew-core.yml` summary output and bot token/fork variables.
5. Security failures: inspect `.github/workflows/sec-audit.yml` and `deny.toml`.
6. Workflow syntax/lint failures: inspect `.github/workflows/workflow-sanity.yml`.
7. PR intake failures: inspect `.github/workflows/pr-intake-checks.yml` sticky comment and run logs.
8. Label policy parity failures: inspect `.github/workflows/pr-label-policy-check.yml`.
9. Docs failures in CI: inspect `docs-quality` job logs in `.github/workflows/ci-run.yml`.
10. Strict delta lint failures in CI: inspect `lint-strict-delta` job logs and compare with `BASE_SHA` diff scope.

## Maintenance Rules

- Keep merge-blocking checks deterministic and reproducible (`--locked` where applicable).
- Follow [`docs/contributing/release-process.md`](./release-process.md) for verify-before-publish release cadence and tag discipline.
- Keep merge-blocking rust quality policy aligned across `.github/workflows/ci-run.yml`, `dev/ci.sh`, and `.githooks/pre-push` (`./scripts/ci/rust_quality_gate.sh` + `./scripts/ci/rust_strict_delta_gate.sh`).
- Use `./scripts/ci/rust_strict_delta_gate.sh` (or `./dev/ci.sh lint-delta`) as the incremental strict merge gate for changed Rust lines.
- Run full strict lint audits regularly via `./scripts/ci/rust_quality_gate.sh --strict` (for example through `./dev/ci.sh lint-strict`) and track cleanup in focused PRs.
- Keep docs markdown gating incremental via `./scripts/ci/docs_quality_gate.sh` (block changed-line issues, report baseline issues separately).
- Keep docs link gating incremental via `./scripts/ci/collect_changed_links.py` + lychee (check only links added on changed lines).
- Prefer explicit workflow permissions (least privilege).
- Keep Actions source policy restricted to approved allowlist patterns (see [`docs/contributing/actions-source-policy.md`](./actions-source-policy.md)).
- Use path filters for expensive workflows when practical.
- Keep docs quality checks low-noise (incremental markdown + incremental added-link checks).
- Keep dependency update volume controlled (grouping + PR limits).
- Avoid mixing onboarding/community automation with merge-gating logic.

## Automation Side-Effect Controls

- Prefer deterministic automation that can be manually overridden (`risk: manual`) when context is nuanced.
- Keep auto-response comments deduplicated to prevent triage noise.
- Keep auto-close behavior scoped to issues; maintainers own PR close/merge decisions.
- If automation is wrong, correct labels first, then continue review with explicit rationale.
- Use `superseded` / `stale-candidate` labels to prune duplicate or dormant PRs before deep review.
