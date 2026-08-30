# Fenrir engineering guide

## Purpose

Fenrir searches versioned workload and fault genomes against external target contracts.
Search code must not depend on target internals.
Synthetic adapters are explicit model or oracle targets, never substitutes for canonical real-target evidence.

## Boundaries

- `fenrir-core` owns genomes, objectives, observations, and counterexamples.
- `fenrir-search` owns optimizers, archives, novelty, and checkpoint state.
- `fenrir-runner` owns local and Kiln execution semantics.
- `fenrir-targets` owns target contract mappings.
- `fenrir-store` owns append-only campaign evidence and manifests.
- `fenrir-cli` composes these boundaries and keeps diagnostics on stderr.

## Invariants

- A genome dimension must change decoded target behavior or campaign validation fails.
- Choice values decode through their declared value table, never their coordinate by accident.
- Seed replay reproduces generated choices. It does not promise equivalent timing measurements.
- Resume restores optimizer and RNG state before suggesting another candidate.
- Counterexample minimization preserves the failure oracle and the original artifact.
- Real and synthetic target names remain visibly distinct in stored evidence.

## Verification

Use `./scripts/verify` for the functional suite.
Use `./scripts/confidence` for formatting, Clippy, all-feature tests, and a release build.

## Lab-wide contracts

- See https://github.com/stra-ta/.github/blob/main/LAB_RULES.md and https://github.com/stra-ta/.github/blob/main/EVIDENCE.md and https://github.com/stra-ta/.github/blob/main/COMPATIBILITY.md for lab-wide naming, evidence, and schema contracts.
- Per https://github.com/stra-ta/.github/blob/main/CONTRIBUTING.md, contributions require the target repo's AGENTS.md, README, and relevant design note, preserve repo boundaries, add the narrowest regression test, run one-command verification, and keep performance claims tied to committed manifests.
