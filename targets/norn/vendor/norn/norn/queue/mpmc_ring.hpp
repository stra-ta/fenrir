#pragma once

#include <array>
#include <atomic>
#include <cstddef>
#include <limits>
#include <new>
#include <optional>
#include <type_traits>
#include <utility>

#include "norn/core/atomic.hpp"

namespace norn {

namespace detail {

// Classify the modular difference `sequence - position`.
// Returns 0 when equal, 1 when the sequence is ahead of the position (stale
// view), and -1 when it is behind (slot not available in this pass).
// Legitimate differences stay far below half the size_t range because the
// queue capacity is constrained to that bound.
[[nodiscard]] inline int sequence_relation(std::size_t sequence, std::size_t position) noexcept {
  const std::size_t diff = sequence - position;
  if (diff == 0) {
    return 0;
  }
  constexpr std::size_t half = std::size_t{1} << (std::numeric_limits<std::size_t>::digits - 1);
  return diff < half ? 1 : -1;
}

}  // namespace detail

// Bounded MPMC queue using the Vyukov sequence-number ring scheme.
// Producers claim slots with a CAS on enqueue_pos_ and consumers claim slots
// with a CAS on dequeue_pos_, and each slot's sequence number publishes the
// payload with release semantics and publishes slot reuse with release too.
// The queue is mutex-free and non-blocking, but not formally lock-free: a
// preempted thread holding a reserved-but-unpublished slot blocks logical
// progress through the queue.
// See docs/MPMC_DESIGN.md for the full design and its documented limits.
template <typename T, std::size_t Capacity>
class mpmc_ring {
  static_assert(Capacity >= 2, "bounded MPMC capacity must be at least 2");
  static_assert(
      Capacity < (std::size_t{1} << (std::numeric_limits<std::size_t>::digits - 1)),
      "bounded MPMC capacity must stay below half the size_t range");
  static_assert(
      std::atomic<std::size_t>::is_always_lock_free,
      "mpmc_ring requires lock-free position and sequence atomics");
  static_assert(
      std::is_nothrow_destructible_v<T>,
      "mpmc_ring requires a noexcept destructor for T");

  struct slot {
    std::atomic<std::size_t> sequence;
    alignas(T) std::byte storage[sizeof(T)];
  };

 public:
  mpmc_ring() {
    for (std::size_t i = 0; i < Capacity; ++i) {
      slots_[i].sequence.store(i, std::memory_order_relaxed);
    }
  }

  mpmc_ring(const mpmc_ring&) = delete;
  mpmc_ring& operator=(const mpmc_ring&) = delete;

  ~mpmc_ring() { clear(); }

  bool try_push(const T& value) { return emplace(value); }

  bool try_push(T&& value) { return emplace(std::move(value)); }

  template <typename... Args>
  bool emplace(Args&&... args) {
    static_assert(
        std::is_nothrow_constructible_v<T, Args...>,
        "mpmc_ring requires noexcept construction of T");
    std::size_t pos = enqueue_pos_.value.load(std::memory_order_relaxed);
    slot* cell = nullptr;
    for (;;) {
      cell = &slots_[pos % Capacity];
      const std::size_t seq = cell->sequence.load(std::memory_order_acquire);
      const int relation = detail::sequence_relation(seq, pos);
      if (relation == 0) {
        if (enqueue_pos_.value.compare_exchange_strong(
                pos, pos + 1, std::memory_order_relaxed)) {
          break;
        }
        // CAS failure reloaded pos with the current counter value.
        continue;
      }
      if (relation < 0) {
        return false;
      }
      pos = enqueue_pos_.value.load(std::memory_order_relaxed);
    }
    ::new (static_cast<void*>(cell->storage)) T(std::forward<Args>(args)...);
    cell->sequence.store(pos + 1, std::memory_order_release);
    return true;
  }

  bool try_pop(T& value) {
    static_assert(
        std::is_nothrow_move_assignable_v<T>,
        "mpmc_ring requires a nothrow move assignment for T");
    std::size_t pos = dequeue_pos_.value.load(std::memory_order_relaxed);
    slot* cell = nullptr;
    for (;;) {
      cell = &slots_[pos % Capacity];
      const std::size_t seq = cell->sequence.load(std::memory_order_acquire);
      const int relation = detail::sequence_relation(seq, pos + 1);
      if (relation == 0) {
        if (dequeue_pos_.value.compare_exchange_strong(
                pos, pos + 1, std::memory_order_relaxed)) {
          break;
        }
        continue;
      }
      if (relation < 0) {
        return false;
      }
      pos = dequeue_pos_.value.load(std::memory_order_relaxed);
    }
    T* element = pointer_at(pos);
    value = std::move(*element);
    element->~T();
    cell->sequence.store(pos + Capacity, std::memory_order_release);
    return true;
  }

  [[nodiscard]] std::optional<T> try_pop() {
    static_assert(
        std::is_nothrow_move_constructible_v<T>,
        "mpmc_ring requires a nothrow move constructor for T");
    std::size_t pos = dequeue_pos_.value.load(std::memory_order_relaxed);
    slot* cell = nullptr;
    for (;;) {
      cell = &slots_[pos % Capacity];
      const std::size_t seq = cell->sequence.load(std::memory_order_acquire);
      const int relation = detail::sequence_relation(seq, pos + 1);
      if (relation == 0) {
        if (dequeue_pos_.value.compare_exchange_strong(
                pos, pos + 1, std::memory_order_relaxed)) {
          break;
        }
        continue;
      }
      if (relation < 0) {
        return std::nullopt;
      }
      pos = dequeue_pos_.value.load(std::memory_order_relaxed);
    }
    T* element = pointer_at(pos);
    // Finish the move before ending the slot's lifetime. The move is
    // statically nothrow, so the object is always in a valid state here.
    std::optional<T> result(std::in_place, std::move(*element));
    element->~T();
    cell->sequence.store(pos + Capacity, std::memory_order_release);
    return result;
  }

  // Reservation-gap snapshot, not an availability check.
  // enqueue_pos_ advances at reservation time, so the counters can differ
  // while no item is actually pop-able (a producer inside the
  // reservation-to-publication window). See docs/MPMC_DESIGN.md.
  [[nodiscard]] bool empty() const noexcept {
    return dequeue_pos_.value.load(std::memory_order_relaxed) ==
           enqueue_pos_.value.load(std::memory_order_relaxed);
  }

  [[nodiscard]] static constexpr std::size_t capacity() noexcept { return Capacity; }

 private:
  T* pointer_at(std::size_t pos) noexcept {
    return std::launder(reinterpret_cast<T*>(slots_[pos % Capacity].storage));
  }

  // Destroys published-but-unclaimed objects. Requires quiescence: no thread
  // may be inside an operation when this runs, matching the SPSC contract.
  void clear() noexcept {
    const std::size_t read = dequeue_pos_.value.load(std::memory_order_relaxed);
    const std::size_t write = enqueue_pos_.value.load(std::memory_order_relaxed);
    for (std::size_t pos = read; pos != write; ++pos) {
      pointer_at(pos)->~T();
    }
  }

  std::array<slot, Capacity> slots_{};
  isolated_atomic<std::size_t> enqueue_pos_{};
  isolated_atomic<std::size_t> dequeue_pos_{};
};

}  // namespace norn
