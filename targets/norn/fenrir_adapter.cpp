#include <norn/mpmc_queue.hpp>
#include <norn/hazard_pointer.hpp>
#include <nlohmann/json.hpp>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <memory>
#include <numeric>
#include <optional>
#include <random>
#include <string>
#include <thread>
#include <vector>

#ifdef __linux__
#include <pthread.h>
#include <sched.h>
#endif
#ifdef __APPLE__
#include <mach/mach.h>
#include <mach/thread_act.h>
#include <mach/thread_policy.h>
#include <pthread.h>
#endif

using json = nlohmann::json;

/// Shared write counter. Writes bump this atomic, modelling the coherence
/// traffic a real write path incurs (a persisted/shared structure being
/// updated). Under many consumers this makes write-heavy workloads measurably
/// more contended than read-heavy ones.
static std::atomic<uint64_t> g_write_ops{0};

/// Maximum payload bytes an item can carry. The genome's `payload.size`
/// choice tops out at 1024, so this bounds per-element memory in the ring.
static constexpr size_t PAYLOAD_CAP = 1024;

/// Ring element. Every field is meaningful to the workload:
///   size      - how many bytes of `data` are touched by producer/consumer
///   is_write  - whether the consumer treats the item as a write (mutating)
///   data      - the payload buffer (only `size` bytes are used)
struct Payload {
    uint32_t size = 0;
    uint32_t is_write = 0;
    // Steady-clock timestamp (ns) captured by the producer at the moment the
    // item is enqueued. The consumer subtracts it to measure residence time:
    // how long the item waited in the ring before being serviced. This is the
    // latency signal burstiness drives (floods pile items up; steady rates
    // don't), and it is immune to producer scheduling artefacts.
    uint64_t enqueue_ns = 0;
    std::array<uint8_t, PAYLOAD_CAP> data;
};

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

    struct PayloadCfg {
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
    uint64_t seed = 0;
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
        cfg.payload.size = static_cast<int>(p.value("size", 192.0));
        cfg.payload.key_distribution = p.value("key_distribution", "uniform");
    }
    if (genome_json.contains("behavior")) {
        auto& b = genome_json["behavior"];
        cfg.behavior.read_write_ratio = b.value("read_write_ratio", 0.5);
        cfg.behavior.lifetime_ns = b.value("lifetime_ns", int64_t{10000});
        cfg.behavior.think_time_ns = b.value("think_time_ns", int64_t{0});
    }
    cfg.duration_sec = genome_json.value("duration_sec", 10);
    cfg.warmup_sec = genome_json.value("warmup_sec", 2);
    cfg.output_path = genome_json.value("output_path", "");
    cfg.seed = genome_json.value("seed", 0ULL);
    return cfg;
}

/// Pin the calling thread according to the genome's affinity policy.
///
///   none         - do not pin (OS schedules freely)
///   core         - one distinct physical core per thread (SMT siblings avoided)
///   socket       - keep every thread on socket 0
///   smt_sibling  - round-robin over all logical CPUs (pairs SMT siblings)
void apply_affinity(const std::string& policy, int thread_id) {
    int n = static_cast<int>(std::thread::hardware_concurrency());
    if (n <= 0) n = 1;
    if (policy == "none") {
        return;
    }

    int cpu = 0;
    if (policy == "socket") {
        cpu = 0;
    } else if (policy == "smt_sibling") {
        cpu = thread_id % n;
    } else if (policy == "core") {
        int physical = n / 2;
        if (physical <= 0) physical = n;
        cpu = thread_id % physical;
    } else {
        cpu = thread_id % n;
    }
    if (cpu < 0) cpu = 0;
    cpu %= n;

#ifdef __linux__
    cpu_set_t cpuset;
    CPU_ZERO(&cpuset);
    CPU_SET(cpu, &cpuset);
    pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
#elif defined(__APPLE__)
    thread_affinity_policy_data_t policy_data;
    policy_data.affinity_tag = static_cast<integer_t>(cpu);
    thread_port_t threadport = pthread_mach_thread_np(pthread_self());
    thread_policy_set(threadport, THREAD_AFFINITY_POLICY,
                      reinterpret_cast<thread_policy_t>(&policy_data),
                      THREAD_AFFINITY_POLICY_COUNT);
#else
    (void)cpu;
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
    apply_affinity(cfg.topology.affinity, thread_id);

    std::mt19937_64 rng(cfg.seed ^ (static_cast<uint64_t>(thread_id) * 0x9E3779B97F4A7C15ULL));
    std::normal_distribution<double> normal_dist(0.0, 1.0);
    std::uniform_real_distribution<double> uniform01(0.0, 1.0);

    auto start = std::chrono::steady_clock::now();
    auto end_time = start + std::chrono::seconds(cfg.duration_sec + cfg.warmup_sec);

    const double lambda_total = std::max(1.0, cfg.traffic.rate);
    const double per_producer =
        lambda_total / static_cast<double>(std::max(1, cfg.topology.producers));
    const double burst = cfg.traffic.burstiness;
    const double corr = cfg.traffic.correlation;
    // Burst intensity: busy epochs run at K/(1 - burst) x mean while occupying
    // only (1 - burst)/K of the time, keeping mean load constant. K>1 makes
    // bursts intense enough to overflow the ring and create real contention.
    const double burst_intensity = 6.0;

    // Burst state machine: busy epochs run above the mean rate while idle
    // epochs pause production, preserving the mean offered load (see below).
    bool burst_state = true;
    // AR(1) state for inter-arrival autocorrelation.
    double ar1 = 0.0;
    uint64_t counter = static_cast<uint64_t>(thread_id) * 2654435761ULL;

    // Per-producer phase offset so producers don't wake in lockstep (which
    // would create artificial synchronized-contention spikes unrelated to the
    // genome).
    auto next_arrival = start + std::chrono::nanoseconds(
        static_cast<int64_t>(uniform01(rng) * 5000000.0));
    auto epoch_end = next_arrival;

    while (running.load(std::memory_order_acquire) &&
           std::chrono::steady_clock::now() < end_time) {

        auto now = std::chrono::steady_clock::now();

        // --- epoch transitions (time-based, mean-preserving) ---
        // Busy epochs run at the mean rate scaled by 1/(1 - burst); idle
        // epochs pause production. The fraction of time spent busy is
        // (1 - burst) and the busy rate is 1/(1 - burst) times the mean, so
        // the *mean* offered load is independent of burstiness: burstiness
        // only concentrates the same load into spikes.
        if (now >= epoch_end) {
            burst_state = !burst_state;
            // Busy fraction of time is (1 - burst)/K and the busy rate is
            // K/(1 - burst) times the mean, so the *mean* offered load stays
            // constant. K>1 makes bursts intense enough to overflow the ring
            // and create real contention spikes; burst=0 is constant rate.
            double f = (burst <= 0.0) ? 1.0 : (1.0 - burst) / burst_intensity;
            double mean_busy_ms = 5.0;
            double mean_idle_ms = mean_busy_ms * (1.0 - f) / f;
            double mean_ms = burst_state ? mean_busy_ms : mean_idle_ms;
            double u = uniform01(rng);
            double ms = -mean_ms * std::log(1.0 - u);  // exponential(ms)
            epoch_end = now + std::chrono::microseconds(static_cast<int64_t>(ms * 1000.0));
            next_arrival = now;
        }

        if (!burst_state) {
            // Idle epoch: wait it out, emit nothing.
            auto wait = epoch_end - std::chrono::steady_clock::now();
            if (wait > std::chrono::milliseconds(0)) {
                std::this_thread::sleep_for(wait);
            }
            continue;
        }

        // --- schedule the next arrival (busy epoch) ---
        // Burst-free mode paces Poisson arrivals (rate + correlation matter).
        // Bursty mode floods at hardware speed during busy epochs so the queue
        // actually overflows and produces real contention spikes (OS sleep
        // granularity would otherwise cap the burst rate at the drain rate).
        double mult = (burst <= 0.0) ? 1.0 : burst_intensity / std::max(1e-3, 1.0 - burst);
        double base_gap = 1.0 / (per_producer * mult);
        double eps = normal_dist(rng);
        ar1 = corr * ar1 + std::sqrt(std::max(0.0, 1.0 - corr * corr)) * eps;
        double gap = base_gap * (1.0 + corr * ar1) * (0.5 + uniform01(rng));
        if (!(gap > 1e-9)) gap = 1e-9;
        if (gap > base_gap * 50.0) gap = base_gap * 50.0;

        if (burst <= 0.0) {
            next_arrival += std::chrono::nanoseconds(static_cast<int64_t>(gap * 1e9));
            auto sched = next_arrival;
            if (sched >= epoch_end) {
                continue;  // no room left in this busy epoch; let it end
            }
            now = std::chrono::steady_clock::now();
            if (sched > now) {
                std::this_thread::sleep_until(sched);
            }
        }
        // burst > 0 busy: intentionally no pacing sleep -> flood

        // --- build a payload of the genome-requested size ---
        size_t sz = static_cast<size_t>(cfg.payload.size);
        if (sz < 1) sz = 1;
        if (sz > PAYLOAD_CAP) sz = PAYLOAD_CAP;
        bool is_write = uniform01(rng) < cfg.behavior.read_write_ratio;

        T item;
        item.size = static_cast<uint32_t>(sz);
        item.is_write = is_write ? 1u : 0u;
        uint8_t fill = static_cast<uint8_t>(counter & 0xFFu);
        for (size_t i = 0; i < sz; ++i) {
            item.data[i] = static_cast<uint8_t>(fill ^ static_cast<uint8_t>(i));
        }
        counter = counter * 6364136223846793005ULL + 1442695040888963407ULL;

        // --- enqueue ---
        // Stamp the enqueue time on each attempt so the value that lands in the
        // ring is the timestamp of the successful push. The consumer measures
        // residence time (enqueue -> dequeue), which is the latency burstiness
        // drives -- and which is unaffected by how the producer itself paces.
        bool pushed = false;
        uint64_t retries = 0;
        while (!pushed && running.load(std::memory_order_acquire)) {
            item.enqueue_ns = static_cast<uint64_t>(
                std::chrono::duration_cast<std::chrono::nanoseconds>(
                    std::chrono::steady_clock::now().time_since_epoch()).count());
            if (queue.try_push(item)) {
                pushed = true;
            } else {
                retries++;
                std::this_thread::yield();
            }
        }

        if (pushed) {
            stats.ops.fetch_add(1, std::memory_order_relaxed);
            stats.cas_retries.fetch_add(retries, std::memory_order_relaxed);
        }

        if (cfg.behavior.think_time_ns > 0) {
            // Busy-wait for the same timer-granularity reason as the consumer
            // service time: sleep_for would not honour sub-millisecond delays.
            auto think_end = std::chrono::steady_clock::now() +
                std::chrono::nanoseconds(cfg.behavior.think_time_ns);
            while (std::chrono::steady_clock::now() < think_end) {}
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
    apply_affinity(cfg.topology.affinity, thread_id);

    auto start = std::chrono::steady_clock::now();
    auto end_time = start + std::chrono::seconds(cfg.duration_sec + cfg.warmup_sec);
    auto measurement_start = start + std::chrono::seconds(cfg.warmup_sec);

    while (running.load(std::memory_order_acquire) &&
           std::chrono::steady_clock::now() < end_time) {

        std::optional<T> result;
        uint64_t retries = 0;
        while (!result.has_value() && running.load(std::memory_order_acquire)) {
            result = queue.try_pop();
            if (!result.has_value()) {
                retries++;
                std::this_thread::yield();
            }
        }

        if (result.has_value()) {
            T item = std::move(result).value();

            // Service work scales with payload size and read/write semantics.
            // Reads are pure consumption; writes mutate the payload, compute a
            // checksum over it, and bump a shared atomic (modelling the
            // coherence traffic of a real write path). Writes are therefore
            // materially more expensive than reads, so read_write_ratio is a
            // real control knob rather than a label.
            size_t sz = static_cast<size_t>(item.size);
            if (sz > PAYLOAD_CAP) sz = PAYLOAD_CAP;
            if (item.is_write == 1u) {
                for (size_t i = 0; i < sz; ++i) {
                    item.data[i] = static_cast<uint8_t>(item.data[i] ^ 0x5Au);
                }
                uint64_t ck = 14695981039346656037ULL; // FNV-1a offset basis
                for (size_t i = 0; i < sz; ++i) {
                    ck = ck ^ static_cast<uint64_t>(item.data[i]);
                    ck = ck * 1099511628211ULL;
                }
                g_write_ops.fetch_add(1, std::memory_order_relaxed);
                (void)ck;
            } else {
                uint64_t sum = 0;
                for (size_t i = 0; i < sz; ++i) {
                    sum += static_cast<uint64_t>(item.data[i]);
                }
                (void)sum;
            }

            // `lifetime_ns` is the consumer-side service time: it throttles
            // how fast the queue drains, which feeds back into producer
            // contention. Real and distinct from producer `think_time_ns`.
            // Busy-wait (not sleep_for): on macOS and many kernels sleep_for
            // has millisecond-scale timer coalescing, so a 1us service sleep
            // would throttle the consumer far below its intended drain rate and
            // confound every downstream latency signal with OS timer noise.
            if (cfg.behavior.lifetime_ns > 0) {
                auto svc_end = std::chrono::steady_clock::now() +
                    std::chrono::nanoseconds(cfg.behavior.lifetime_ns);
                while (std::chrono::steady_clock::now() < svc_end) {}
            }

            // Residence-time latency: how long this item waited in the ring
            // before the consumer serviced it. This is what burstiness controls
            // (floods back the queue up; steady rates don't).
            uint64_t recv_ns = static_cast<uint64_t>(
                std::chrono::duration_cast<std::chrono::nanoseconds>(
                    std::chrono::steady_clock::now().time_since_epoch()).count());
            if (recv_ns > item.enqueue_ns) {
                uint64_t latency = recv_ns - item.enqueue_ns;
                if (std::chrono::steady_clock::now() > measurement_start) {
                    std::lock_guard<std::mutex> lock(stats.latencies_mutex);
                    stats.latencies.push_back(latency);
                }
            }

            stats.ops.fetch_add(1, std::memory_order_relaxed);
            stats.cas_retries.fetch_add(retries, std::memory_order_relaxed);
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
    return std::accumulate(data.begin(), data.end(), 0.0) /
           static_cast<double>(data.size());
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

    using PayloadT = Payload;
    // Allocate the ring on the heap: 8192 elements of a 1 KiB payload exceed
    // the default thread stack. A unique_ptr keeps the storage off the stack.
    auto queue = std::make_unique<norn::mpmc_ring<PayloadT, 8192>>();

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
        producers.emplace_back(producer_thread<PayloadT>,
                               std::ref(*queue),
                               std::ref(producer_stats[static_cast<size_t>(i)]),
                               std::cref(cfg),
                               i,
                               std::ref(running));
    }

    for (int i = 0; i < cfg.topology.consumers; ++i) {
        consumers.emplace_back(consumer_thread<PayloadT>,
                               std::ref(*queue),
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
        std::lock_guard<std::mutex> lock(s.latencies_mutex);
        all_latencies.insert(all_latencies.end(), s.latencies.begin(), s.latencies.end());
    }
    for (auto& s : consumer_stats) {
        total_consumer_ops += s.ops.load();
        std::lock_guard<std::mutex> lock(s.latencies_mutex);
        all_latencies.insert(all_latencies.end(), s.latencies.begin(), s.latencies.end());
    }

    double measured_sec = std::max(1.0, static_cast<double>(cfg.duration_sec));
    // Throughput is the drain rate (items actually serviced), not the sum of
    // producer pushes and consumer pops -- counting both would double-count each
    // item.
    double throughput = static_cast<double>(total_consumer_ops) / measured_sec;
    double cas_retries_per_op = total_producer_ops > 0
        ? static_cast<double>(total_cas_retries) / static_cast<double>(total_producer_ops)
        : 0.0;

    double p50 = percentile(all_latencies, 0.50);
    double p90 = percentile(all_latencies, 0.90);
    double p95 = percentile(all_latencies, 0.95);
    double p99 = percentile(all_latencies, 0.99);
    double p999 = percentile(all_latencies, 0.999);
    double max_lat = all_latencies.empty()
        ? 0.0
        : static_cast<double>(*std::max_element(all_latencies.begin(), all_latencies.end()));

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
