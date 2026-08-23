// Loki Fenrir adapter: self-contained fault-injection proxy simulator.
//
// Reads a genome JSON on stdin (--genome -) and prints ONE metrics JSON object
// on stdout. No external Loki binary is required: the adapter derives a fault
// schedule from the genome and simulates a proxy applying those faults to a
// stream of upstream operations, measuring degraded throughput and elevated
// tail latency.
//
// Genome knobs -> simulator behavior:
//   topology.producers / topology.consumers -> proxy worker (connection) count
//   traffic.rate        -> upstream target throughput (ops/s, Poisson arrivals)
//   traffic.burstiness  -> arrival burstiness (compress inter-arrival gaps)
//   traffic.correlation -> couples reorder probability
//   payload.size        -> per-op processing cost
//   behavior.read_write_ratio -> scales injected latency magnitude
//   behavior.think_time_ns    -> baseline per-op service floor
//   faults.fault_probability  -> P(op gets a latency-injection fault)
//   faults.latency_mean_ns    -> mean injected delay (exponential around mean)
//   faults.reorder_prob       -> P(op is reordered/delayed)
//   faults.blackhole_prob     -> P(op is silently dropped / never acked)

#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <iostream>
#include <queue>
#include <random>
#include <string>
#include <vector>

using json = nlohmann::json;

struct Config {
    struct Topology {
        int producers = 1;
        int consumers = 1;
        std::string affinity = "core";
    } topology;

    struct Traffic {
        std::string arrival_process = "poisson";
        double rate = 1'000'000.0;
        double burstiness = 0.0;
        double correlation = 0.0;
    } traffic;

    struct Payload {
        std::string size_distribution = "fixed";
        int size = 256;
        std::string key_distribution = "uniform";
    } payload;

    struct Behavior {
        double read_write_ratio = 0.2;
        int64_t lifetime_ns = 100'000;
        int64_t think_time_ns = 500;
    } behavior;

    struct Faults {
        double fault_probability = 0.0;
        double latency_mean_ns = 0.0;
        double reorder_prob = 0.0;
        double blackhole_prob = 0.0;
    } faults;

    int duration_sec = 10;
    int warmup_sec = 2;
};

Config parse_genome(const json& genome_json) {
    Config cfg;
    if (genome_json.contains("topology")) {
        auto& t = genome_json["topology"];
        cfg.topology.producers = t.value("producers", 1);
        cfg.topology.consumers = t.value("consumers", 1);
        cfg.topology.affinity = t.value("affinity", "core");
    }
    if (genome_json.contains("traffic")) {
        auto& t = genome_json["traffic"];
        cfg.traffic.arrival_process = t.value("arrival_process", "poisson");
        cfg.traffic.rate = t.value("rate", 1'000'000.0);
        cfg.traffic.burstiness = t.value("burstiness", 0.0);
        cfg.traffic.correlation = t.value("correlation", 0.0);
    }
    if (genome_json.contains("payload")) {
        auto& p = genome_json["payload"];
        cfg.payload.size_distribution = p.value("size_distribution", "fixed");
        cfg.payload.size = p.value("size", 256);
        cfg.payload.key_distribution = p.value("key_distribution", "uniform");
    }
    if (genome_json.contains("behavior")) {
        auto& b = genome_json["behavior"];
        cfg.behavior.read_write_ratio = b.value("read_write_ratio", 0.2);
        cfg.behavior.lifetime_ns = b.value("lifetime_ns", 100'000);
        cfg.behavior.think_time_ns = b.value("think_time_ns", 500);
    }
    if (genome_json.contains("faults")) {
        auto& f = genome_json["faults"];
        cfg.faults.fault_probability = f.value("fault_probability", 0.0);
        cfg.faults.latency_mean_ns = f.value("latency_mean_ns", 0.0);
        cfg.faults.reorder_prob = f.value("reorder_prob", 0.0);
        cfg.faults.blackhole_prob = f.value("blackhole_prob", 0.0);
    }
    cfg.duration_sec = genome_json.value("duration_sec", 10);
    cfg.warmup_sec = genome_json.value("warmup_sec", 2);
    return cfg;
}

double percentile(std::vector<uint64_t>& data, double p) {
    if (data.empty()) return 0.0;
    std::sort(data.begin(), data.end());
    size_t idx = static_cast<size_t>(p * static_cast<double>(data.size() - 1));
    return static_cast<double>(data[idx]);
}

int main(int argc, char** argv) {
    if (argc < 2) {
        std::cerr << "Usage: " << argv[0]
                  << " --genome <genome.json> [--output <output.json>] [--duration <sec>]\n";
        return 1;
    }

    std::string genome_path;
    int duration_sec = 10;

    for (int i = 1; i < argc; ++i) {
        std::string arg = argv[i];
        if (arg == "--genome" && i + 1 < argc) {
            genome_path = argv[++i];
        } else if (arg == "--duration" && i + 1 < argc) {
            duration_sec = std::stoi(argv[++i]);
        }
    }

    if (genome_path.empty()) {
        std::cerr << "Error: --genome required\n";
        return 1;
    }

    json genome_json;
    if (genome_path == "-") {
        std::cin >> genome_json;
    } else {
        std::cerr << "Error: only --genome - (stdin) is supported\n";
        return 1;
    }

    Config cfg = parse_genome(genome_json);
    cfg.duration_sec = duration_sec;

    // ---- Deterministic seeded RNG (seeded from the genome for repeatability) ----
    uint64_t seed = 0x10c0ffee;
    seed ^= static_cast<uint64_t>(cfg.traffic.rate);
    seed ^= static_cast<uint64_t>(cfg.faults.fault_probability * 1'000'000.0);
    seed ^= static_cast<uint64_t>(cfg.faults.latency_mean_ns);
    std::mt19937_64 rng(seed);
    auto uniform = [&]() -> double {
        return std::uniform_real_distribution<double>(0.0, 1.0)(rng);
    };

    // ---- Simulator parameters ----
    const int workers = std::clamp(cfg.topology.producers * cfg.topology.consumers, 1, 256);
    const double rate = std::max(cfg.traffic.rate, 1.0);          // ops/s target
    const double burstiness = std::clamp(cfg.traffic.burstiness, 0.0, 1.0);
    const double correlation = std::clamp(cfg.traffic.correlation, 0.0, 1.0);
    const double read_write_ratio = std::clamp(cfg.behavior.read_write_ratio, 0.0, 1.0);

    const double fault_probability = std::clamp(cfg.faults.fault_probability, 0.0, 1.0);
    const double latency_mean_ns = std::max(cfg.faults.latency_mean_ns, 0.0);
    // correlation raises the effective reorder probability.
    const double reorder_prob = std::clamp(
        cfg.faults.reorder_prob * (0.5 + 0.5 * correlation), 0.0, 1.0);
    const double blackhole_prob = std::clamp(cfg.faults.blackhole_prob, 0.0, 1.0);

    // base service time per op: processing cost + pacing floor from think_time.
    const double base_service_ns =
        50.0 + 0.25 * static_cast<double>(cfg.payload.size) +
        0.05 * static_cast<double>(cfg.behavior.think_time_ns);
    // read/write ratio slightly amplifies injected latency magnitude.
    const double latency_scale = 1.0 + 0.5 * read_write_ratio;

    const int64_t warmup_ns = static_cast<int64_t>(cfg.warmup_sec) * 1'000'000'000LL;
    const int64_t total_ns =
        static_cast<int64_t>(cfg.warmup_sec + cfg.duration_sec) * 1'000'000'000LL;
    const double measure_sec = std::max(1.0, static_cast<double>(cfg.duration_sec));

    // ---- Simulated M/G/k proxy with fault injection ----
    std::priority_queue<int64_t, std::vector<int64_t>, std::greater<int64_t>> worker_free;
    for (int i = 0; i < workers; ++i) worker_free.push(0);

    std::vector<uint64_t> latencies;
    latencies.reserve(2'000'000);

    uint64_t injected_faults = 0;
    uint64_t reorder_count = 0;
    uint64_t blackhole_drops = 0;
    uint64_t ack_count = 0;
    double latency_injected_sum = 0.0;
    uint64_t latency_injected_n = 0;

    const double mean_inter_ns = 1'000'000'000.0 / rate;
    double arrival_ns = 0.0;

    std::exponential_distribution<double> exp_inter(1.0 / mean_inter_ns);

    while (arrival_ns < static_cast<double>(total_ns)) {
        const double a = arrival_ns;

        // Decide faults for this op.
        bool blackholed = uniform() < blackhole_prob;

        double injected = 0.0;
        bool faulted = false;
        if (!blackholed && uniform() < fault_probability) {
            faulted = true;
            double mean = std::max(latency_mean_ns * latency_scale, 1.0);
            injected = std::exponential_distribution<double>(1.0 / mean)(rng);
        }

        double reorder_extra = 0.0;
        bool reordered = false;
        if (!blackholed && uniform() < reorder_prob) {
            reordered = true;
            double jitter = (uniform() * 2.0 - 1.0) * 0.1 * std::max(latency_mean_ns, 1.0);
            reorder_extra = std::max(jitter, 0.0);
        }

        bool is_fault = blackholed || faulted || reordered;
        if (is_fault) ++injected_faults;
        if (reordered) ++reorder_count;
        if (blackholed) {
            ++blackhole_drops;
        } else {
            int64_t start = static_cast<int64_t>(a);
            int64_t wfree = worker_free.top();
            worker_free.pop();
            if (wfree > start) start = wfree;
            int64_t svc = static_cast<int64_t>(
                base_service_ns + injected + reorder_extra);
            if (svc < 1) svc = 1;
            int64_t finish = start + svc;
            worker_free.push(finish);

            if (finish <= total_ns) ++ack_count;
            if (a >= static_cast<double>(warmup_ns)) {
                double lat = static_cast<double>(finish - static_cast<int64_t>(a));
                if (lat < 0.0) lat = 0.0;
                latencies.push_back(static_cast<uint64_t>(lat));
            }
        }
        if (faulted) {
            latency_injected_sum += injected;
            ++latency_injected_n;
        }

        // Next arrival (Poisson). Apply burstiness by compressing some gaps.
        double inter = exp_inter(rng);
        if (burstiness > 0.0 && uniform() < burstiness) inter *= 0.1;
        arrival_ns += inter;
    }

    double throughput = static_cast<double>(ack_count) / measure_sec;

    double p50 = percentile(latencies, 0.50);
    double p90 = percentile(latencies, 0.90);
    double p95 = percentile(latencies, 0.95);
    double p99 = percentile(latencies, 0.99);
    double p999 = percentile(latencies, 0.999);
    double max_lat = latencies.empty()
        ? 0.0
        : static_cast<double>(*std::max_element(latencies.begin(), latencies.end()));
    double latency_injected_ns =
        latency_injected_n > 0 ? latency_injected_sum / static_cast<double>(latency_injected_n) : 0.0;

    json observation;
    observation["metrics"]["application"]["throughput_ops_s"] = throughput;
    observation["metrics"]["application"]["p50_latency_ns"] = p50;
    observation["metrics"]["application"]["p90_latency_ns"] = p90;
    observation["metrics"]["application"]["p95_latency_ns"] = p95;
    observation["metrics"]["application"]["p99_latency_ns"] = p99;
    observation["metrics"]["application"]["p999_latency_ns"] = p999;
    observation["metrics"]["application"]["max_latency_ns"] = max_lat;
    observation["metrics"]["application"]["injected_faults"] =
        static_cast<double>(injected_faults);
    observation["metrics"]["application"]["reorder_count"] =
        static_cast<double>(reorder_count);
    observation["metrics"]["application"]["blackhole_drops"] =
        static_cast<double>(blackhole_drops);
    observation["metrics"]["application"]["latency_injected_ns"] = latency_injected_ns;

    observation["metrics"]["hardware"]["cycles"] = nullptr;
    observation["metrics"]["os"]["context_switches"] = nullptr;

    std::cout << observation.dump(2) << "\n";
    return 0;
}
