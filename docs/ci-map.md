# ZeroClaw CI/CD Map

This document maps repository paths to the GitHub Actions workflows that govern them.

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

1. `CI` failing: Inspect `.github/workflows/ci.yml`.
2. Release build failures: Inspect `.github/workflows/release-build.yml`.
3. Release failures: Inspect `.github/workflows/release.yml`.
