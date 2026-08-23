#include <norn/mpmc_queue.hpp>
#include <norn/hazard_pointer.hpp>
#include <nlohmann/json.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <numeric>
#include <optional>
#include <random>
#include <thread>
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
        double rate = 1000000.0;
        double burstiness = 0.0;
        double correlation = 0.0;
    } traffic;

    struct Payload {
        std::string size_distribution = "fixed";
        int size = 192;
        std::string key_distribution = "uniform";
    } payload;

    struct Behavior {
        double read_write_ratio = 0.5;
        int64_t lifetime_ns = 10000;
        int64_t think_time_ns = 0;
    } behavior;

    int duration_sec = 10;
    int warmup_sec = 2;
    std::string output_path;
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
        cfg.payload.size = p.value("size", 192);
        cfg.payload.key_distribution = p.value("key_distribution", "uniform");
    }
    if (genome_json.contains("behavior")) {
        auto& b = genome_json["behavior"];
        cfg.behavior.read_write_ratio = b.value("read_write_ratio", 0.5);
        cfg.behavior.lifetime_ns = b.value("lifetime_ns", 10000);
        cfg.behavior.think_time_ns = b.value("think_time_ns", 0);
    }
    cfg.duration_sec = genome_json.value("duration_sec", 10);
    cfg.warmup_sec = genome_json.value("warmup_sec", 2);
    cfg.output_path = genome_json.value("output_path", "");
    return cfg;
}

void pin_thread_to_cpu(int cpu_id) {
#if defined(__linux__)
    cpu_set_t cpuset;
    CPU_ZERO(&cpuset);
    CPU_SET(cpu_id % std::thread::hardware_concurrency(), &cpuset);
    pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
#elif defined(__APPLE__)
    (void)cpu_id;
#endif
}

struct alignas(64) ThreadStats {
    std::atomic<uint64_t> ops{0};
    std::atomic<uint64_t> cas_retries{0};
    std::vector<uint64_t> latencies;
    std::mutex latencies_mutex;
};

template <typename T>
void producer_thread(
    norn::mpmc_ring<T, 8192>& queue,
    ThreadStats& stats,
    const Config& cfg,
    int thread_id,
    std::atomic<bool>& running
) {
    pin_thread_to_cpu(thread_id);

    std::mt19937_64 rng(static_cast<uint64_t>(
        std::chrono::steady_clock::now().time_since_epoch().count() + thread_id
    ));

    auto start = std::chrono::steady_clock::now();
    auto end_time = start + std::chrono::seconds(cfg.duration_sec + cfg.warmup_sec);

    uint64_t value = static_cast<uint64_t>(thread_id);
    while (running.load(std::memory_order_acquire) &&
           std::chrono::steady_clock::now() < end_time) {

        auto op_start = std::chrono::steady_clock::now();
        bool pushed = false;
        uint64_t retries = 0;

        T payload;
        payload.fill(static_cast<uint8_t>(value & 0xFF));

        while (!pushed && running.load(std::memory_order_acquire)) {
            if (queue.try_push(payload)) {
                pushed = true;
                value += static_cast<uint64_t>(cfg.topology.producers);
            } else {
                retries++;
                std::this_thread::yield();
            }
        }

        auto op_end = std::chrono::steady_clock::now();
        auto latency = std::chrono::duration_cast<std::chrono::nanoseconds>(op_end - op_start).count();

        if (pushed) {
            stats.ops.fetch_add(1, std::memory_order_relaxed);
            stats.cas_retries.fetch_add(retries, std::memory_order_relaxed);
            if (std::chrono::steady_clock::now() > start + std::chrono::seconds(cfg.warmup_sec)) {
                std::lock_guard lock(stats.latencies_mutex);
                stats.latencies.push_back(static_cast<uint64_t>(latency));
            }
        }

        if (cfg.behavior.think_time_ns > 0) {
            std::this_thread::sleep_for(std::chrono::nanoseconds(cfg.behavior.think_time_ns));
        }
    }
}

template <typename T>
void consumer_thread(
    norn::mpmc_ring<T, 8192>& queue,
    ThreadStats& stats,
    const Config& cfg,
    int thread_id,
    std::atomic<bool>& running
) {
    pin_thread_to_cpu(thread_id);

    auto start = std::chrono::steady_clock::now();
    auto end_time = start + std::chrono::seconds(cfg.duration_sec + cfg.warmup_sec);

    while (running.load(std::memory_order_acquire) &&
           std::chrono::steady_clock::now() < end_time) {

        auto op_start = std::chrono::steady_clock::now();
        std::optional<T> result;
        uint64_t retries = 0;

        while (!result.has_value() && running.load(std::memory_order_acquire)) {
            result = queue.try_pop();
            if (!result.has_value()) {
                retries++;
                std::this_thread::yield();
            }
        }

        auto op_end = std::chrono::steady_clock::now();
        auto latency = std::chrono::duration_cast<std::chrono::nanoseconds>(op_end - op_start).count();

        if (result.has_value()) {
            stats.ops.fetch_add(1, std::memory_order_relaxed);
            stats.cas_retries.fetch_add(retries, std::memory_order_relaxed);
            if (std::chrono::steady_clock::now() > start + std::chrono::seconds(cfg.warmup_sec)) {
                std::lock_guard lock(stats.latencies_mutex);
                stats.latencies.push_back(static_cast<uint64_t>(latency));
            }
        }
    }
}

double percentile(std::vector<uint64_t>& data, double p) {
    if (data.empty()) return 0.0;
    std::sort(data.begin(), data.end());
    size_t idx = static_cast<size_t>(p * static_cast<double>(data.size() - 1));
    return static_cast<double>(data[idx]);
}

double mean(const std::vector<uint64_t>& data) {
    if (data.empty()) return 0.0;
    return std::accumulate(data.begin(), data.end(), 0.0) / static_cast<double>(data.size());
}

int main(int argc, char** argv) {
    if (argc < 2) {
        std::cerr << "Usage: " << argv[0] << " --genome <genome.json> [--output <output.json>] [--duration <sec>]\n";
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

    using Payload = std::array<uint8_t, 256>;
    norn::mpmc_ring<Payload, 8192> queue;

    std::vector<std::thread> producers;
    std::vector<std::thread> consumers;
    std::vector<ThreadStats> producer_stats(static_cast<size_t>(cfg.topology.producers));
    std::vector<ThreadStats> consumer_stats(static_cast<size_t>(cfg.topology.consumers));

    std::atomic<bool> running{true};

    std::cerr << "Starting Norn MPMC benchmark: "
              << cfg.topology.producers << " producers, "
              << cfg.topology.consumers << " consumers, "
              << "duration=" << cfg.duration_sec << "s\n";

    for (int i = 0; i < cfg.topology.producers; ++i) {
        producers.emplace_back(producer_thread<Payload>,
                              std::ref(queue),
                              std::ref(producer_stats[static_cast<size_t>(i)]),
                              std::cref(cfg),
                              i,
                              std::ref(running));
    }

    for (int i = 0; i < cfg.topology.consumers; ++i) {
        consumers.emplace_back(consumer_thread<Payload>,
                              std::ref(queue),
                              std::ref(consumer_stats[static_cast<size_t>(i)]),
                              std::cref(cfg),
                              i,
                              std::ref(running));
    }

    std::this_thread::sleep_for(std::chrono::seconds(cfg.duration_sec + cfg.warmup_sec));
    running.store(false, std::memory_order_release);

    for (auto& t : producers) t.join();
    for (auto& t : consumers) t.join();

    uint64_t total_producer_ops = 0;
    uint64_t total_consumer_ops = 0;
    uint64_t total_cas_retries = 0;
    std::vector<uint64_t> all_latencies;

    for (auto& s : producer_stats) {
        total_producer_ops += s.ops.load();
        total_cas_retries += s.cas_retries.load();
        std::lock_guard lock(s.latencies_mutex);
        all_latencies.insert(all_latencies.end(), s.latencies.begin(), s.latencies.end());
    }
    for (auto& s : consumer_stats) {
        total_consumer_ops += s.ops.load();
        std::lock_guard lock(s.latencies_mutex);
        all_latencies.insert(all_latencies.end(), s.latencies.begin(), s.latencies.end());
    }

    double measured_sec = std::max(1.0, static_cast<double>(cfg.duration_sec));
    double throughput = static_cast<double>(total_producer_ops + total_consumer_ops) / measured_sec;
    double cas_retries_per_op = total_producer_ops > 0 ?
        static_cast<double>(total_cas_retries) / static_cast<double>(total_producer_ops) : 0.0;

    double p50 = percentile(all_latencies, 0.50);
    double p90 = percentile(all_latencies, 0.90);
    double p95 = percentile(all_latencies, 0.95);
    double p99 = percentile(all_latencies, 0.99);
    double p999 = percentile(all_latencies, 0.999);
    double max_lat = all_latencies.empty() ? 0.0 : static_cast<double>(*std::max_element(all_latencies.begin(), all_latencies.end()));

    json observation;
    observation["run_id"] = "manual";
    observation["timestamp_ns"] = std::chrono::duration_cast<std::chrono::nanoseconds>(
        std::chrono::system_clock::now().time_since_epoch()).count();
    observation["duration_ns"] = static_cast<int64_t>(measured_sec * 1e9);
    observation["status"] = "success";

    observation["metrics"]["application"]["throughput_ops_s"] = throughput;
    observation["metrics"]["application"]["p50_latency_ns"] = p50;
    observation["metrics"]["application"]["p90_latency_ns"] = p90;
    observation["metrics"]["application"]["p95_latency_ns"] = p95;
    observation["metrics"]["application"]["p99_latency_ns"] = p99;
    observation["metrics"]["application"]["p999_latency_ns"] = p999;
    observation["metrics"]["application"]["max_latency_ns"] = max_lat;
    observation["metrics"]["application"]["cas_retries_per_op"] = cas_retries_per_op;
    observation["metrics"]["application"]["producer_ops"] = total_producer_ops;
    observation["metrics"]["application"]["consumer_ops"] = total_consumer_ops;

    observation["metrics"]["hardware"]["cycles"] = nullptr;
    observation["metrics"]["hardware"]["instructions"] = nullptr;
    observation["metrics"]["hardware"]["cache_misses"] = nullptr;
    observation["metrics"]["os"]["context_switches"] = nullptr;
    observation["metrics"]["os"]["rss_kb"] = nullptr;

    if (!output_path.empty()) {
        std::ofstream out(output_path);
        out << observation.dump(2);
    } else {
        std::cout << observation.dump(2) << "\n";
    }

    return 0;
}