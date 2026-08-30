# Fenrir

Adversarial workload synthesis engine.

Fenrir searches for pathological workloads that push target systems into their worst-case behavior: tail-latency blowups, contention collapses, fault-injection degradation, order-book thrash.

It treats workload generation as a search problem over a "genome" (a structured space of workload parameters) against a configurable objective, and drives each candidate through a target adapter that actually runs the workload and reports measurements.

## Status

This is a v0 prototype.

The Norn target adapter links the real Norn library.
The Weir, Loki, and Orda adapters are **faithful self-contained C++ simulators** of each target's documented adversarial surface; they do not yet link the real libraries.
Ygg behavioral embeddings (novelty search) are not yet wired in.

Campaigns are reproducible: `--seed <n>` flows into the optimizer, workload
generation, and adapter threads, so the same seed replays the same workloads
(observed latencies carry real-system timing noise). A canonical head-to-head
run (random vs CMA-ES vs MAP-Elites, same space/budget/seed against Norn) lives
in [`experiments/canonical/`](experiments/canonical/README.md) and shows Fenrir
discovering pathological multi-second tail latencies.

## Architecture

Rust is the control plane: campaign orchestration, search algorithms, persistence, and the target/trait boundary.
C++ is the target plane: hot workload generators compiled into small adapter binaries.

```
fenrir/
├── crates/
│   ├── fenrir-core/        # Genome, Objective, Observation, SearchSpace
│   ├── fenrir-targets/     # Target trait + per-target Rust modules
│   ├── fenrir-runner/      # Local subprocess backend + Kiln backend
│   ├── fenrir-search/      # Random, CMA-ES, (Bayesian stub), MAP-Elites
│   ├── fenrir-store/       # JSONL campaign persistence
│   └── fenrir-cli/         # CLI entry point
├── targets/
│   ├── norn/               # C++ adapter (links real Norn) + CMake
│   ├── weir/               # C++ adapter (synthetic simulator) + CMake
│   ├── loki/               # C++ adapter (synthetic fault proxy) + CMake
│   └── orda/               # C++ adapter (synthetic matching engine) + CMake
└── experiments/            # Campaign outputs (gitignored)
```

Each target provides two things: a Rust module implementing the `Target` trait, and a C++ adapter binary that reads a genome on stdin and prints metrics on stdout.

## Build

### Rust

```bash
cargo build --release
# or just the CLI
cargo build -p fenrir-cli
```

### C++ target adapters

Each adapter builds with CMake into `targets/<name>/build/<name>_fenrir_adapter`:

```bash
cd targets/norn && cmake -S . -B build && cmake --build build
cd targets/weir && cmake -S . -B build && cmake --build build
cd targets/loki && cmake -S . -B build && cmake --build build
cd targets/orda && cmake -S . -B build && cmake --build build
```

The adapters use `nlohmann_json` (resolved via CMake `find_package`) and threads only.
They are self-contained and need no external target library to build or run.

## CLI

### `fenrir hunt`

Search for pathological workloads.

| Flag | Default | Meaning |
|------|---------|---------|
| `--target` | `norn` | Target system: `norn`, `weir`, `loki`, `orda` |
| `-g, --genome` | — | Genome TOML (defaults to the target's `default_genome`) |
| `-j, --objective` | — | Objective TOML (defaults to `p99_latency`) |
| `--optimizer` | `random` | `random`, `cmaes`, `bayesian`, `map-elites` |
| `--budget` | `100` | Total evaluations |
| `--parallel` | `1` | Parallel evaluations |
| `--duration` | `10` | Seconds per evaluation |
| `--seed` | `42` | RNG seed for optimizer, workload generation, and adapter threads |
| `-o, --output` | `experiments/fenrir_hunt` | Campaign directory |
| `--adapter` | — | Override adapter binary path |
| `--backend` | `local` | `local` (subprocess) or `kiln` (Kiln runner) |

Examples:

```bash
# Norn, CMA-ES
fenrir hunt --target norn --optimizer cmaes --budget 40 --duration 5

# Weir, random
fenrir hunt --target weir --budget 20 --duration 3

# Orda via the Kiln backend
fenrir hunt --target orda --budget 20 --duration 3 --backend kiln

# Quality-diversity search with MAP-Elites
fenrir hunt --target norn --optimizer map-elites --budget 60 --duration 3
```

### `fenrir analyze`

Summarize a finished campaign.

```bash
fenrir analyze experiments/fenrir_hunt --top 10
```

## Targets

| Target | Adapter | Models |
|--------|---------|--------|
| `norn` | links real Norn `mpmc_ring` | MPMC queue contention, CAS retries, latency under producer/consumer pressure |
| `weir` | synthetic simulator | Durable event gateway: WAL group-commit saturation, connection storms, deep pipelines, payload cost, epoll vs io_uring |
| `loki` | synthetic fault proxy | TCP fault injection: latency, reorder, blackhole; degraded throughput and tail |
| `orda` | synthetic matching engine | Order-book thrash: add/cancel ratio, price-locality hotspots, churn, book depletion |

Each target defines its own `default_genome()` (the search space) and maps sampled genomes to the JSON its adapter expects.

## Adapter protocol

The runner invokes each adapter as:

```bash
<adapter> --genome - --duration <SECONDS>
```

The genome JSON is written to the adapter's stdin.
The adapter prints a single JSON object to stdout:

```json
{
  "metrics": {
    "application": {
      "throughput_ops_s": 1234567.0,
      "p50_latency_ns": 800.0,
      "p90_latency_ns": 1200.0,
      "p95_latency_ns": 1500.0,
      "p99_latency_ns": 2200.0,
      "p999_latency_ns": 4000.0,
      "max_latency_ns": 9000.0,
      "cas_retries_per_op": 0.01
    },
    "hardware": { "cycles": null },
    "os": { "context_switches": null }
  }
}
```

Rules the adapter must follow:

- Emit `throughput_ops_s` plus `p50` through `p999` latency (`_ns` suffix).
- `metrics.hardware` and `metrics.os` are optional; `null` values are skipped.
- `throughput_ops_s` must be finite.
- The default objectives require `throughput_ops_s >= 1_000_000` to count an evaluation as feasible, so favorable genomes should reach that range.

## Adding a target

1. Create `crates/fenrir-targets/src/<name>.rs` implementing `Target`:

   ```rust
   pub struct NameTarget { adapter_path: PathBuf }
   impl NameTarget { pub fn new(adapter_path: PathBuf) -> Self { ... } }
   impl Target for NameTarget {
       fn name(&self) -> &str { "name" }
       fn default_genome(&self) -> Genome { /* target-appropriate Genome */ }
       fn adapter_path(&self) -> PathBuf { self.adapter_path.clone() }
       fn genome_to_adapter_input(&self, genome: &SampledGenome) -> serde_json::Value { /* JSON for the adapter */ }
   }
   ```

2. Create `targets/<name>/fenrir_adapter.cpp` + `CMakeLists.txt` following the protocol above (copy `targets/norn/fenrir_adapter.cpp` as a starting point).

3. Register the target in `crates/fenrir-targets/src/lib.rs` `get_target()`.

## Scoring

`Objective` turns an `Observation` into a `Score`.
Higher `Score` means more pathological (worse) behavior; Fenrir's optimizers maximize it.
Constraints (e.g. `throughput_ops_s >= 1_000_000`) mark an evaluation infeasible, which serializes as a distinct sentinel and is excluded from feasible rankings.
Built-in objectives: `p99_latency`, `cas_retries`, `tail_amplification`.

## Optimizers

| Optimizer | Status |
|-----------|--------|
| `random` | Working |
| `cmaes` | Working (native CMA-ES with alternating large/small-population restarts (BIPOP-inspired)) |
| `bayesian` | Placeholder (falls back to random) |
| `map-elites` | Working (MAP-Elites quality-diversity over throughput x tail-amplification) |

## Roadmap

- Wire Ygg embeddings for novelty search and richer MAP-Elites features.
- Replace the Weir/Loki/Orda synthetic simulators with adapters that drive the real libraries (e.g. `weir_bench`, the Orda order book, the Loki proxy) for joint fault + workload search.
- Promote `bayesian` from placeholder to a real surrogate-model optimizer.

## License

[MIT](LICENSE).
