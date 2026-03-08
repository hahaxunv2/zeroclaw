# Workflow Directory Layout

GitHub Actions only loads workflow entry files from:

- `.github/workflows/*.yml`
- `.github/workflows/*.yaml`

Subdirectories are not valid locations for workflow entry files.

Repository convention:

1. Keep runnable workflow entry files at `.github/workflows/` root.
2. Keep cross-tooling/local CI scripts under `dev/` or `scripts/ci/` when used outside Actions.

Workflow behavior documentation in this directory:

<<<<<<< HEAD
- `.github/workflows/main-branch-flow.md`

Current workflow helper scripts:

- `.github/workflows/scripts/ci_license_file_owner_guard.js`
- `.github/workflows/scripts/lint_feedback.js`
- `.github/workflows/scripts/pr_auto_response_contributor_tier.js`
- `.github/workflows/scripts/pr_auto_response_labeled_routes.js`
- `.github/workflows/scripts/pr_check_status_nudge.js`
- `.github/workflows/scripts/pr_intake_checks.js`
- `.github/workflows/scripts/pr_labeler.js`
- `.github/workflows/scripts/test_benchmarks_pr_comment.js`

Release/CI policy assets introduced for advanced delivery lanes:

- `.github/release/nightly-owner-routing.json`
- `.github/release/canary-policy.json`
- `.github/release/prerelease-stage-gates.json`
=======
- `.github/workflows/master-branch-flow.md`
>>>>>>> fa2faf40 (chore: update .gitignore, CODEOWNERS, and dependabot configuration)
