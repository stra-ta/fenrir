# Fenrir

Search for workloads that make a target system behave badly.

![Search strategy, workload genome, runner, target adapter, and recorded evidence](docs/SEARCH.svg)

Fenrir treats workload generation as optimization.
A search strategy proposes a versioned genome, the runner executes it through a target adapter, and the measured observation becomes a score, archive entry, and checkpoint.

## Current status

- Random search, CMA-ES, and MAP-Elites work
- Resume restores optimizer and RNG state before the next candidate
- Counterexample minimization preserves the failure oracle and source artifact
- The Norn adapter links the real library
- Weir, Loki, and Orda currently use explicit synthetic adapters
- Bayesian search and Ygg novelty integration are not implemented

Seed replay reproduces generated choices.
It does not promise identical timing measurements.

[Build adapters, run campaigns, inspect the CLI, and add targets](GUIDE.md).

[Canonical optimizer comparison](experiments/canonical/README.md).
