#pragma once

#include <cstddef>
#include <thread>

#include "norn/core/cpu_relax.hpp"

namespace norn::backoff {

// Internal seam for deterministic tests: policies take their primitive wait
// actions from Ops so tests can count calls instead of observing timing.
// In production Ops is system_wait and every call inlines to cpu_relax() or
// std::this_thread::yield(); there is no virtual dispatch or stored state.
namespace detail {

struct system_wait {
  static void relax() noexcept { norn::cpu_relax(); }
  static void yield() noexcept { std::this_thread::yield(); }
};

template <typename Ops>
struct tight_policy {
  void reset() noexcept {}
  void operator()() noexcept { Ops::relax(); }
};

template <typename Ops>
struct yield_policy {
  void reset() noexcept {}
  void operator()() noexcept { Ops::yield(); }
};

template <typename Ops>
struct bounded_policy {
  void reset() noexcept { count_ = 0; }
  void operator()() noexcept {
    if (count_ < 63) {
      Ops::relax();
    } else {
      Ops::yield();
    }
    ++count_;
    if (count_ >= 64) {
      count_ = 0;
    }
  }

private:
  std::size_t count_ = 0;
};

template <typename Ops>
struct exponential_policy {
  void reset() noexcept {
    step_ = 0;
    failure_count_ = 0;
  }
  void operator()() noexcept {
    const std::size_t spin_count = std::size_t{1} << step_;
    for (std::size_t i = 0; i < spin_count; ++i) {
      Ops::relax();
    }
    ++failure_count_;
    if (failure_count_ >= 64) {
      Ops::yield();
      reset();
      return;
    }
    if (step_ < 6) {
      ++step_;
    }
  }

private:
  std::size_t step_ = 0;
  std::size_t failure_count_ = 0;
};

}  // namespace detail

using tight = detail::tight_policy<detail::system_wait>;
using yield = detail::yield_policy<detail::system_wait>;
using bounded = detail::bounded_policy<detail::system_wait>;
using exponential = detail::exponential_policy<detail::system_wait>;

}  // namespace norn::backoff
