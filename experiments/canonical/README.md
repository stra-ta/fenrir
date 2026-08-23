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
| random | 30 | 30 | 10 | 4974.49 ms | 1843.41 ms | 665 ops/s |
| cmaes | 30 | 30 | 3 | 4975.88 ms | 4754.44 ms | 895 ops/s |
| map-elites | 30 | 30 | 7 | 4993.38 ms | 2471.99 ms | 2147 ops/s |

## What it shows

Within 30 evaluations of a 3-second hunt, every optimizer drives Norn into
visibly pathological regimes: tail latencies in the milliseconds-to-seconds
range with throughput far below the nominal drain rate. The scientific
distinction is *coverage*, not just peak:

- **random** samples the space but lands in few behavioural cells (10 distinct regimes).
- **CMA-ES** climbs toward a single high-value mode (3 cells, best p99 4975.88 ms).
- **MAP-Elites** illuminates the behavioural map, covering 7 distinct (throughput, tail-latency) regimes —
  it trades peak score for a *map* of how Norn fails, which is the quality-
  diversity payoff.

## Most pathological workload discovered

- discovered by optimizer: **map-elites**
- p99 latency: 4993.38 ms (p50 3567.93 ms, p999 5021.33 ms)
- throughput: 2147 ops/s
- genome: `{"topology.consumers": 17.037692736191886, "topology.producers": 13.081014151250768, "behavior.read_write_ratio": 0.9999170512059781, "behavior.think_time_ns": 6254.858050996204, "traffic.burstiness": 0.6544814359827837, "payload.size": 2.0, "traffic.correlation": 0.477495206854075, "traffic.rate": 10000.0, "behavior.lifetime_ns": 2595311.79590334}`

Raw evaluations: `experiments/canonical/{random,cmaes,map-elites}/evaluations.jsonl`.
Reproduce exactly:
```bash
cargo run -p fenrir-cli -- hunt --target norn --optimizer <opt> \
  --budget 30 --duration 3 --seed 42 \
  --objective experiments/canonical/canonical_objective.toml \
  --output experiments/canonical/<opt>
```
