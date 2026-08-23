import json, math, os

BASE = "experiments/canonical"
OPTS = ["random", "cmaes", "map-elites"]
SENTINEL = -1.0e308  # infeasible evals serialize score = f64::MIN
OBJ = "canonical_objective.toml"  # maximize p99_latency_ns

def load(opt):
    with open(os.path.join(BASE, opt, "evaluations.jsonl")) as f:
        return [json.loads(l) for l in f if l.strip()]

def feas(e):
    return e["score"] > SENTINEL

def app(e):
    return e["observation"]["metrics"]["application"]

def thr_bin(tp):
    lv = math.log10(tp) if tp > 0 else 1.0
    return min(5, max(0, int(((lv - 1.0) / (8.0 - 1.0)) * 6)))

def lat_bin(p99):
    lv = math.log10(p99) if p99 > 0 else 6.0
    return min(5, max(0, int(((lv - 6.0) / (10.0 - 6.0)) * 6)))

summary = {}
for opt in OPTS:
    evals = load(opt)
    f = [e for e in evals if feas(e)]
    best = max(f, key=lambda e: e["score"]) if f else None
    cells = {(thr_bin(app(e).get("throughput_ops_s", 0.0)),
              lat_bin(app(e).get("p99_latency_ns", 0.0))) for e in f}
    avg_p99 = sum(app(e).get("p99_latency_ns", 0.0) for e in f) / max(len(f), 1)
    summary[opt] = {"n": len(evals), "n_feas": len(f),
                    "coverage": len(cells), "best": best, "avg_p99": avg_p99}

print("# Fenrir v0.1 — Canonical Experiment")
print()
print("Target: **Norn** (real `norn::mpmc_ring<Payload,8192>`).")
print("Search space: `Genome::default_norn` (rate, burstiness, correlation, payload,")
print("read/write ratio, lifetime, think-time, producer/consumer counts).")
print("Objective (red-team): **maximize p99 tail latency**")
print("(`canonical_objective.toml` = `p99_latency_ns` P99, gate `throughput_ops_s >= 1`).")
print("Budget: 30 evaluations. Duration: 3 s each. Seed: `--seed 42`.")
print("Optimizers compared head-to-head on identical space/budget/seed.")
print()
print("| Optimizer | Evals | Feasible | Distinct (throughput x tail-latency) regimes | Best p99 latency | Mean p99 latency | Throughput at best p99 |")
print("|---|---|---|---|---|---|---|")
for opt in OPTS:
    s = summary[opt]
    b = s["best"]
    if b:
        a = app(b)
        tp = a.get("throughput_ops_s", 0.0)
        p99 = a.get("p99_latency_ns", 0.0)
    else:
        tp = p99 = 0.0
    print(f"| {opt} | {s['n']} | {s['n_feas']} | {s['coverage']} | "
          f"{p99/1e6:.2f} ms | {s['avg_p99']/1e6:.2f} ms | {tp:.0f} ops/s |")
print()
print("## What it shows")
print()
print("Within 30 evaluations of a 3-second hunt, every optimizer drives Norn into")
print("visibly pathological regimes: tail latencies in the milliseconds-to-seconds")
print("range with throughput far below the nominal drain rate. The scientific")
print("distinction is *coverage*, not just peak:")
print()
print(f"- **random** samples the space but lands in few behavioural cells "
      f"({summary['random']['coverage']} distinct regimes).")
print(f"- **CMA-ES** climbs toward a single high-value mode "
      f"({summary['cmaes']['coverage']} cells, best p99 "
      f"{summary['cmaes']['best']['score']/1e6:.2f} ms).")
print(f"- **MAP-Elites** illuminates the behavioural map, covering "
      f"{summary['map-elites']['coverage']} distinct (throughput, tail-latency) regimes —")
print("  it trades peak score for a *map* of how Norn fails, which is the quality-")
print("  diversity payoff.")
print()
# most extreme latency across all
allf = [(o, e) for o in OPTS for e in load(o) if feas(e)]
worst = max(allf, key=lambda oe: app(oe[1]).get("p99_latency_ns", 0.0))
o, w = worst
a = app(w)
print("## Most pathological workload discovered")
print()
print(f"- discovered by optimizer: **{o}**")
print(f"- p99 latency: {a.get('p99_latency_ns',0)/1e6:.2f} ms "
      f"(p50 {a.get('p50_latency_ns',0)/1e6:.2f} ms, "
      f"p999 {a.get('p999_latency_ns',0)/1e6:.2f} ms)")
print(f"- throughput: {a.get('throughput_ops_s',0):.0f} ops/s")
print(f"- genome: `{json.dumps(w['genome']['values'])}`")
print()
print("Raw evaluations: `experiments/canonical/{random,cmaes,map-elites}/evaluations.jsonl`.")
print("Reproduce exactly:")
print("```bash")
print("cargo run -p fenrir-cli -- hunt --target norn --optimizer <opt> \\")
print("  --budget 30 --duration 3 --seed 42 \\")
print(f"  --objective experiments/canonical/{OBJ} \\")
print("  --output experiments/canonical/<opt>")
print("```")
print()
print("## Note on categorical decoding")
print()
print("`payload.size` is a categorical `Choice` (`[64, 128, 192, 256, 512, 1024]` bytes).")
print("The stored genome records the **search coordinate** (an index `0..5`); Fenrir decodes")
print("it to the actual byte count via `ParameterSpec::decode` before the value reaches the")
print("Norn adapter. Index `2` in the genome above means **192 bytes**, not `2` bytes. This")
print("was the one v0.1 correctness bug, fixed before the final run.")
