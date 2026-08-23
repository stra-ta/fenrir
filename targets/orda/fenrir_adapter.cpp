// Orda Fenrir adapter: a self-contained, faithful *synthetic* model of an
// Orda-style matching engine / order book. No real Orda library is required.
//
// It reads a genome JSON on stdin and prints ONE JSON object on stdout:
//   { "metrics": { "application": {...}, "hardware": {"cycles": null},
//                  "os": {"context_switches": null} } }
//
// Genome knob -> Orda knob mapping (mirrors fenrir-targets/src/orda.rs):
//   topology.producers       -> symbol count
//   topology.consumers       -> matching-thread count
//   traffic.rate             -> order ingress rate (ops/s)
//   payload.size             -> price locality mode (0=uniform,1=clustered,2=zipfian)
//   behavior.read_write_ratio-> add/cancel ratio (1.0=all adds, 0.0=all cancels)
//   behavior.lifetime_ns     -> order lifetime
//   behavior.think_time_ns   -> inter-order think time
//   traffic.burstiness       -> price-level churn (0..1)
//   traffic.correlation      -> market/limit mix (0..1)

#include <nlohmann/json.hpp>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <random>
#include <vector>

using json = nlohmann::json;

struct Config {
    struct Topology {
        int producers = 1;      // symbol count
        int consumers = 1;      // matching threads
        std::string affinity = "core";
    } topology;

    struct Traffic {
        std::string arrival_process = "poisson";
        double rate = 1000000.0;     // order ingress rate (ops/s)
        double burstiness = 0.0;     // price-level churn (0..1)
        double correlation = 0.0;    // market/limit mix (0..1)
    } traffic;

    struct Payload {
        std::string size_distribution = "fixed";
        int size = 0;           // locality mode index: 0 uniform,1 clustered,2 zipfian
        std::string key_distribution = "uniform";
    } payload;

    struct Behavior {
        double read_write_ratio = 0.5;  // add/cancel ratio (1=all adds)
        int64_t lifetime_ns = 1000000;  // order lifetime
        int64_t think_time_ns = 0;      // inter-order think time
    } behavior;

    int duration_sec = 10;
    int warmup_sec = 2;
    std::string output_path;
};

static double clampd(double v, double lo, double hi) {
    return std::max(lo, std::min(hi, v));
}

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
        cfg.payload.size = p.value("size", 0);
        cfg.payload.key_distribution = p.value("key_distribution", "uniform");
    }
    if (genome_json.contains("behavior")) {
        auto& b = genome_json["behavior"];
        cfg.behavior.read_write_ratio = b.value("read_write_ratio", 0.5);
        cfg.behavior.lifetime_ns = b.value("lifetime_ns", 1000000);
        cfg.behavior.think_time_ns = b.value("think_time_ns", 0);
    }
    cfg.duration_sec = genome_json.value("duration_sec", 10);
    cfg.warmup_sec = genome_json.value("warmup_sec", 2);
    cfg.output_path = genome_json.value("output_path", "");
    return cfg;
}

// Standard normal sample via Box-Muller.
double gaussian(std::mt19937_64& rng) {
    double u1 = std::uniform_real_distribution<double>(1e-12, 1.0)(rng);
    double u2 = std::uniform_real_distribution<double>(0.0, 1.0)(rng);
    return std::sqrt(-2.0 * std::log(u1)) * std::cos(2.0 * 3.141592653589793 * u2);
}

double percentile(std::vector<double>& data, double p) {
    if (data.empty()) return 0.0;
    std::sort(data.begin(), data.end());
    size_t idx = static_cast<size_t>(p * static_cast<double>(data.size() - 1));
    return data[idx];
}

int main(int argc, char** argv) {
    if (argc < 2) {
        std::cerr << "Usage: " << argv[0]
                  << " --genome <genome.json> [--output <output.json>] [--duration <sec>]\n";
        return 1;
    }

    std::string genome_path;
    std::string output_path;
    int duration_sec = 10;

    for (int i = 1; i < argc; ++i) {
        std::string arg = argv[i];
        if (arg == "--genome" && i + 1 < argc) {
            genome_path = argv[++i];
        } else if (arg == "--output" && i + 1 < argc) {
            output_path = argv[++i];
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

    // --- Resolve Orda knobs -------------------------------------------------
    const int symbols = std::max(1, cfg.topology.producers);          // symbol count
    const int threads = std::max(1, cfg.topology.consumers);          // matching threads
    const double rate = std::max(1.0, cfg.traffic.rate);              // ingress ops/s
    const double churn = clampd(cfg.traffic.burstiness, 0.0, 1.0);    // price-level churn
    const double market_limit_mix = clampd(cfg.traffic.correlation, 0.0, 1.0);
    const int locality = clampd(cfg.payload.size, 0, 2);              // 0/1/2
    const double add_frac = clampd(cfg.behavior.read_write_ratio, 0.0, 1.0);
    const double cancel_frac = 1.0 - add_frac;                        // cancels fraction
    const int64_t lifetime_ns = std::max<int64_t>(1, cfg.behavior.lifetime_ns);
    const int64_t think_time_ns = std::max<int64_t>(0, cfg.behavior.think_time_ns);

    // Price-locality contention factor: clustered and especially zipfian create
    // hotspots on a few price levels -> contention/tail latency.
    double locality_factor;
    if (locality == 0) locality_factor = 1.0;       // uniform
    else if (locality == 1) locality_factor = 3.0;  // clustered
    else locality_factor = 12.0;                    // zipfian (hot price level)

    // Little's law: expected number of resting orders alive in the book.
    const double alive = rate * (static_cast<double>(lifetime_ns) / 1e9);
    // Scaling factor for how much symbol/book pressure amplifies thrash.
    const double turnover_pressure = std::log10(1.0 + alive / 1000.0);

    // THRASH: drives matching-latency blowup. Combination of price-level churn,
    // cancellation ratio, book turnover, and locality hotspots. This is the
    // order-book thrash model: high cancellation + high churn -> book
    // restructuring -> tail latency explosion, while throughput stays high.
    double thrash = churn * (0.4 + cancel_frac) * (1.0 + turnover_pressure)
                   * (locality_factor / 6.0);
    thrash = std::min(thrash, 50.0);

    // Per-op service time (ns). Median-ish matching cost. Throughput capacity
    // is derived from this and is driven mainly by churn + locality (NOT by
    // cancellation alone), so high cancellation + high churn can blow up tail
    // latency *without* necessarily dropping throughput; only extreme churn
    // (combined with high locality contention / few threads) pushes it below 1e6.
    const double base_lat = 40.0;
    const double per_op_lat = base_lat * locality_factor * (1.0 + churn * churn * 60.0);

    // Throughput: min(ingress rate, matching capacity). Capacity = threads *
    // per-thread ops/s where per-thread ops/s = 1e9 / per_op_lat.
    const double per_thread_cap = 1e9 / per_op_lat;
    const double capacity = per_thread_cap * static_cast<double>(threads);
    const double throughput = std::min(rate, capacity);

    // --- Latency samples (synthetic) --------------------------------------
    // Model each order's matching latency as log-normal around per_op_lat, with
    // a heavy tail whose sigma grows with thrash. Add occasional saturation
    // spikes when the hot price level is contended (zipfian + churn).
    const size_t SAMPLES = 400000;
    std::mt19937_64 rng(static_cast<uint64_t>(
        std::chrono::steady_clock::now().time_since_epoch().count()));
    std::uniform_real_distribution<double> u01(0.0, 1.0);

    const double sigma = 0.25 + thrash * 1.2;
    const double mu = std::log(per_op_lat) - 0.5 * sigma * sigma;

    const double sat_prob = 0.001 + 0.05 * churn * (locality == 2 ? 1.0 : 0.3);
    const double sat_spike = per_op_lat * (5.0 + 20.0 * churn);

    std::vector<double> latencies;
    latencies.reserve(SAMPLES);
    for (size_t i = 0; i < SAMPLES; ++i) {
        double lat = std::exp(mu + sigma * gaussian(rng));
        if (u01(rng) < sat_prob) {
            lat += sat_spike * (0.5 + u01(rng));
        }
        // market/limit mix slightly widens the distribution (more diverse flow)
        lat *= (1.0 + 0.1 * market_limit_mix * gaussian(rng) * 0.1);
        if (lat < 1.0) lat = 1.0;
        latencies.push_back(lat);
    }

    const double p50 = percentile(latencies, 0.50);
    const double p90 = percentile(latencies, 0.90);
    const double p95 = percentile(latencies, 0.95);
    const double p99 = percentile(latencies, 0.99);
    const double p999 = percentile(latencies, 0.999);
    const double max_lat = latencies.empty()
        ? 0.0
        : *std::max_element(latencies.begin(), latencies.end());

    // --- Orda-specific scalars --------------------------------------------
    // Actual fraction of orders that are cancels.
    const double cancellation_rate = cancel_frac;

    // Latency incurred when the hot price level saturates (gap blowup).
    const double price_gap_blowup_ns =
        sat_spike * (0.5 + turnover_pressure) * (locality_factor / 6.0);

    // Count (normalized) of empty best-level events: more cancels + more churn
    // + shorter lifetimes empty the book more often; zipfian worsens it.
    const double book_depth_depletion =
        cancel_frac * churn
        * (1.0 / (1.0 + static_cast<double>(lifetime_ns) / 1e6))
        * 1000.0 * (locality == 2 ? 2.0 : 1.0);

    json observation;
    observation["metrics"]["application"]["throughput_ops_s"] = throughput;
    observation["metrics"]["application"]["p50_latency_ns"] = p50;
    observation["metrics"]["application"]["p90_latency_ns"] = p90;
    observation["metrics"]["application"]["p95_latency_ns"] = p95;
    observation["metrics"]["application"]["p99_latency_ns"] = p99;
    observation["metrics"]["application"]["p999_latency_ns"] = p999;
    observation["metrics"]["application"]["max_latency_ns"] = max_lat;
    observation["metrics"]["application"]["cancellation_rate"] = cancellation_rate;
    observation["metrics"]["application"]["price_gap_blowup_ns"] = price_gap_blowup_ns;
    observation["metrics"]["application"]["book_depth_depletion"] = book_depth_depletion;

    observation["metrics"]["hardware"]["cycles"] = nullptr;
    observation["metrics"]["os"]["context_switches"] = nullptr;

    if (!output_path.empty()) {
        std::ofstream out(output_path);
        out << observation.dump(2);
    } else {
        std::cout << observation.dump(2) << "\n";
    }

    return 0;
}
