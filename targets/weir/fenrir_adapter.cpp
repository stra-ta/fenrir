// Weir Fenrir adapter: a self-contained, faithful *synthetic* model of the
// Weir durable event gateway (WR01 frames + WAL with group commit).
//
// No real Weir library is linked. v0 delivers a synthetic simulator whose
// adversarial surfaces are modeled analytically so the Fenrir search has a
// meaningful, fast, deterministic objective landscape.
//
// Genome -> knob mapping (the Rust side emits the SAME document shape as Norn):
//   topology.producers        -> connections (concurrent client connections)
//   topology.consumers        -> window / WAL group-commit pipeline depth
//   traffic.rate              -> rate limit (target ops/s)
//   traffic.burstiness        -> traffic burstiness (tail amplifier)
//   traffic.correlation       -> payload-size correlation (tail variance)
//   payload.size              -> frame payload bytes (copy/serialize cost)
//   behavior.read_write_ratio -> write fraction (WAL sync cost)
//   behavior.lifetime_ns      -> connection lifetime
//   behavior.think_time_ns    -> inter-request think time (gates arrival)
//   topology.affinity         -> backend hint: "core"/"smt_sibling" => io_uring
//                                (low base latency); "socket"/"none" => epoll
//                                (higher base latency).
//
// Adversarial surfaces modeled:
//   * connection storms + deep pipelines   -> WAL group-commit saturation
//                                              -> tail latency blowup
//   * large payloads                       -> copy/serialize cost on median+tail
//   * rate limit near capacity             -> queueing -> p99/p999 spikes
//   * io_uring modeled as lower base latency than epoll

#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <limits>
#include <random>
#include <thread>
#include <vector>

using json = nlohmann::json;

static double clamp_d(double x, double lo, double hi) {
    return std::max(lo, std::min(hi, x));
}

struct Config {
    struct Topology {
        int producers = 1;     // connections
        int consumers = 1;     // window / pipeline depth
        std::string affinity = "core";
    } topology;

    struct Traffic {
        std::string arrival_process = "poisson";
        double rate = 1000000.0;
        double burstiness = 0.0;
        double correlation = 0.0;
    } traffic;

    struct Payload {
        std::string size_distribution = "fixed";
        int size = 1024;
        std::string key_distribution = "uniform";
    } payload;

    struct Behavior {
        double read_write_ratio = 0.5;  // write fraction
        int64_t lifetime_ns = 100000;
        int64_t think_time_ns = 1000;
    } behavior;

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
        cfg.traffic.rate = t.value("rate", 1000000.0);
        cfg.traffic.burstiness = t.value("burstiness", 0.0);
        cfg.traffic.correlation = t.value("correlation", 0.0);
    }
    if (genome_json.contains("payload")) {
        auto& p = genome_json["payload"];
        cfg.payload.size_distribution = p.value("size_distribution", "fixed");
        cfg.payload.size = p.value("size", 1024);
        cfg.payload.key_distribution = p.value("key_distribution", "uniform");
    }
    if (genome_json.contains("behavior")) {
        auto& b = genome_json["behavior"];
        cfg.behavior.read_write_ratio = b.value("read_write_ratio", 0.5);
        cfg.behavior.lifetime_ns = b.value("lifetime_ns", 100000);
        cfg.behavior.think_time_ns = b.value("think_time_ns", 1000);
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
                  << " --genome <genome.json|-> [--duration <sec>]\n";
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
        std::ifstream genome_file(genome_path);
        if (!genome_file) {
            std::cerr << "Error: cannot open genome file: " << genome_path << "\n";
            return 1;
        }
        genome_file >> genome_json;
    }
    Config cfg = parse_genome(genome_json);
    cfg.duration_sec = duration_sec;

    // ----- Model parameters -----
    int connections = std::max(1, cfg.topology.producers);
    int window = std::max(1, cfg.topology.consumers);
    double rate = std::max(1.0, cfg.traffic.rate);
    double size = static_cast<double>(std::max(1, cfg.payload.size));
    double write_frac = clamp_d(cfg.behavior.read_write_ratio, 0.0, 1.0);
    double think_ns = static_cast<double>(std::max(static_cast<int64_t>(1),
                                                   cfg.behavior.think_time_ns));
    double burst = clamp_d(cfg.traffic.burstiness, 0.0, 1.0);
    double corr = clamp_d(cfg.traffic.correlation, 0.0, 1.0);

    // Backend: io_uring (core/smt_sibling) vs epoll (socket/none).
    bool use_io_uring = (cfg.topology.affinity == "core") ||
                        (cfg.topology.affinity == "smt_sibling");
    double backend_base = use_io_uring ? 800.0 : 2500.0;

    // Per-op service time (no WAL).
    double serialize_ns = size * 0.3;  // copy/serialize cost scales with payload
    double base_ns = backend_base + serialize_ns;

    // WAL group-commit sync cost: grows with write fraction and window depth.
    double wal_ns = write_frac * (3000.0 + 4000.0 * (static_cast<double>(window) / 1000.0));
    double eff_service = base_ns + wal_ns;

    // Arrival rate gated by per-connection think time.
    double per_conn_rate = 1e9 / (think_ns + eff_service);  // ops/s per connection
    double arrival_rate = std::min(rate, static_cast<double>(connections) * per_conn_rate);

    // Contention stress: products of normalized connections & window, amplified
    // by burstiness and write fraction (WAL saturation).
    double nrm = static_cast<double>(connections) / 1000.0;
    double wrm = static_cast<double>(window) / 1000.0;
    double stress = nrm * wrm * (1.0 + burst) * (0.5 + 0.5 * write_frac);

    // System capacity collapses under high stress -> infeasible (throughput<1e6).
    const double C_MAX = 50.0e6;
    const double K = 30.0;
    double capacity = C_MAX / (1.0 + stress * K);
    double throughput = std::min(arrival_rate, capacity);

    // Saturation -> connection drops + pipeline stalls.
    double saturation = (arrival_rate > capacity)
                            ? (arrival_rate - capacity) / arrival_rate
                            : 0.0;
    double connection_errors = saturation * static_cast<double>(connections) * 0.5;
    double pipeline_stalls = stress * arrival_rate * 1e-4;
    double wal_sync_latency_ns = wal_ns * (1.0 + stress * 5.0);

    // ----- Synthetic latency samples -----
    // The runner supplies a per-evaluation seed in the genome document.  A
    // fixed fallback keeps direct adapter invocations deterministic too.
    const uint64_t seed = genome_json.value("seed", 0ULL);
    std::mt19937_64 rng(seed);
    const double RNG_MAX = static_cast<double>(std::numeric_limits<uint64_t>::max());

    const int M = 100000;
    std::vector<uint64_t> latencies;
    latencies.reserve(static_cast<size_t>(M));

    double contention_prob = clamp_d(stress * 0.5, 0.0, 0.95);
    double tail_mult = 1.0 + stress * 40.0 + (size / 65536.0) * 30.0 +
                       burst * stress * 60.0 + corr * stress * 20.0;

    for (int i = 0; i < M; ++i) {
        double u = static_cast<double>(rng()) / RNG_MAX;
        double op_base = base_ns * (0.8 + 0.4 * u);  // +-10% service spread
        double lat_ns = op_base;

        // Contended ops suffer a heavy (pow-shaped) tail.
        if ((static_cast<double>(rng()) / RNG_MAX) < contention_prob) {
            double z = static_cast<double>(rng()) / RNG_MAX;
            lat_ns += base_ns * tail_mult * std::pow(z, 4.0);
        }

        // Writes pay WAL sync latency.
        if ((static_cast<double>(rng()) / RNG_MAX) < write_frac) {
            lat_ns += wal_ns * (0.5 + 0.5 * (static_cast<double>(rng()) / RNG_MAX));
        }

        latencies.push_back(static_cast<uint64_t>(std::max(1.0, lat_ns)));
    }

    double p50 = percentile(latencies, 0.50);
    double p90 = percentile(latencies, 0.90);
    double p95 = percentile(latencies, 0.95);
    double p99 = percentile(latencies, 0.99);
    double p999 = percentile(latencies, 0.999);
    double max_lat = latencies.empty() ? 0.0
                                       : static_cast<double>(*std::max_element(
                                             latencies.begin(), latencies.end()));

    // ----- Run the modeled window (mirrors Norn timing) -----
    std::this_thread::sleep_for(
        std::chrono::seconds(cfg.duration_sec + cfg.warmup_sec));

    json observation;
    observation["metrics"]["application"]["throughput_ops_s"] = throughput;
    observation["metrics"]["application"]["p50_latency_ns"] = p50;
    observation["metrics"]["application"]["p90_latency_ns"] = p90;
    observation["metrics"]["application"]["p95_latency_ns"] = p95;
    observation["metrics"]["application"]["p99_latency_ns"] = p99;
    observation["metrics"]["application"]["p999_latency_ns"] = p999;
    observation["metrics"]["application"]["max_latency_ns"] = max_lat;
    observation["metrics"]["application"]["connection_errors"] = connection_errors;
    observation["metrics"]["application"]["wal_sync_latency_ns"] = wal_sync_latency_ns;
    observation["metrics"]["application"]["pipeline_stalls"] = pipeline_stalls;
    observation["metrics"]["hardware"]["cycles"] = nullptr;
    observation["metrics"]["os"]["context_switches"] = nullptr;
    observation["metadata"]["seed"] = seed;
    observation["metadata"]["adapter"] = "weir-synthetic";

    std::cout << observation.dump(2) << "\n";
    return 0;
}
