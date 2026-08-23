# Fenrir v0.1 — Canonical Experiment

Target: **Norn** (real `norn::mpmc_ring<Payload,8192>`).
Search space: `Genome::default_norn` (rate, burstiness, correlation, payload,
read/write ratio, lifetime, think-time, producer/consumer counts).
Objective (red-team): **maximize p99 tail latency**
(`canonical_objective.toml` = `p99_latency_ns` P99, gate `throughput_ops_s >= 1`).
Budget: 30 evaluations. Duration: 3 s each. Seed: `--seed 42`.
Optimizers compared head-to-head on identical space/budget/seed.

| Optimizer | Evals | Feasible | Distinct (throughput x tail-latency) regimes | Best p99 latency | Mean p99 latency | Throughput at best p99 |
|---|---|---|---|---|---|---|
| random | 30 | 30 | 8 | 4978.08 ms | 1782.19 ms | 2270 ops/s |
| cmaes | 30 | 30 | 4 | 4980.24 ms | 4469.97 ms | 1070 ops/s |
| map-elites | 30 | 30 | 8 | 4992.98 ms | 2099.69 ms | 545 ops/s |

## What it shows

Within 30 evaluations of a 3-second hunt, every optimizer drives Norn into
visibly pathological regimes: tail latencies in the milliseconds-to-seconds
range with throughput far below the nominal drain rate. The scientific
distinction is *coverage*, not just peak:

- **random** samples the space but lands in few behavioural cells (8 distinct regimes).
- **CMA-ES** climbs toward a single high-value mode (4 cells, best p99 4980.24 ms).
- **MAP-Elites** illuminates the behavioural map, covering 8 distinct (throughput, tail-latency) regimes —
  it trades peak score for a *map* of how Norn fails, which is the quality-
  diversity payoff.

## Most pathological workload discovered

- discovered by optimizer: **map-elites**
- p99 latency: 4992.98 ms (p50 3404.30 ms, p999 5003.56 ms)
- throughput: 545 ops/s
- genome: `{"traffic.correlation": 0.6567454030189187, "traffic.rate": 54978.99005213576, "behavior.lifetime_ns": 3475770.380938441, "behavior.read_write_ratio": 0.7480579725869084, "topology.consumers": 3.399240993692033, "topology.producers": 18.680821392736647, "traffic.burstiness": 0.7657756291360915, "behavior.think_time_ns": 33.05714310146563, "payload.size": 5.0}`

Raw evaluations: `experiments/canonical/{random,cmaes,map-elites}/evaluations.jsonl`.
Reproduce exactly:
```bash
cargo run -p fenrir-cli -- hunt --target norn --optimizer <opt> \
  --budget 30 --duration 3 --seed 42 \
  --objective experiments/canonical/canonical_objective.toml \
  --output experiments/canonical/<opt>
```
