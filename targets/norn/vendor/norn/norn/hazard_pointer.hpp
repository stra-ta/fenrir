#pragma once

#include <algorithm>
#include <atomic>
#include <cstddef>
#include <memory>
#include <mutex>
#include <new>
#include <optional>
#include <type_traits>
#include <unordered_set>
#include <vector>

#include "norn/cache_line.hpp"

namespace norn {

class hazard_domain;
class hazard_record;

struct hp_retired_entry {
  void* ptr;
  void (*deleter)(void*) noexcept;
};

// Shared thread_local record pointer so register and deregister see the same variable.
namespace detail {
inline thread_local hazard_record* tls_record = nullptr;
}  // namespace detail

// ---------------------------------------------------------------------------
// hazard_record — per-thread hazard slots, registered with a domain
// ---------------------------------------------------------------------------

class hazard_record {
  friend class hazard_domain;

  static constexpr int kSlots = 2;
  std::atomic<void*> slots_[kSlots]{};

 public:
  std::vector<hp_retired_entry> retired_;
  hazard_domain* domain_ = nullptr;

 public:
  hazard_record() = default;

  hazard_record(const hazard_record&) = delete;
  hazard_record& operator=(const hazard_record&) = delete;
  hazard_record(hazard_record&&) = delete;
  hazard_record& operator=(hazard_record&&) = delete;

  template <int SlotIndex, typename U>
  U* protect(std::atomic<void*>& source) {
    static_assert(SlotIndex < kSlots);
    U* ptr = nullptr;
    for (;;) {
      ptr = static_cast<U*>(source.load(std::memory_order_acquire));
      slots_[SlotIndex].store(ptr, std::memory_order_release);
      std::atomic_thread_fence(std::memory_order_seq_cst);
      void* current = source.load(std::memory_order_acquire);
      if (current == ptr) {
        break;
      }
      slots_[SlotIndex].store(nullptr, std::memory_order_relaxed);
      ptr = static_cast<U*>(current);
    }
    return ptr;
  }

  template <int SlotIndex>
  void clear() {
    static_assert(SlotIndex < kSlots);
    slots_[SlotIndex].store(nullptr, std::memory_order_relaxed);
  }

  void retire_entry(void* ptr, void (*deleter)(void*) noexcept) {
    retired_.push_back({ptr, deleter});
  }

  [[nodiscard]] std::size_t retired_count() const noexcept {
    return retired_.size();
  }
};

// ---------------------------------------------------------------------------
// hazard_ptr<T, Index> — RAII guard protecting one pointer
// ---------------------------------------------------------------------------

template <typename T, int Index = 0>
class hazard_ptr {
 public:
  hazard_ptr(hazard_record& rec, std::atomic<void*>& source)
      : record_(rec), value_(rec.protect<Index, T>(source)) {}

  ~hazard_ptr() { record_.template clear<Index>(); }

  hazard_ptr(const hazard_ptr&) = delete;
  hazard_ptr& operator=(const hazard_ptr&) = delete;
  hazard_ptr(hazard_ptr&&) = delete;
  hazard_ptr& operator=(hazard_ptr&&) = delete;

  [[nodiscard]] T* get() const noexcept { return value_; }
  [[nodiscard]] T& operator*() const noexcept { return *value_; }
  [[nodiscard]] T* operator->() const noexcept { return value_; }

 private:
  hazard_record& record_;
  T* value_;
};

// ---------------------------------------------------------------------------
// hazard_domain — shared registration and reclamation state
// ---------------------------------------------------------------------------

class hazard_domain {
 public:
  hazard_domain() = default;

  hazard_domain(const hazard_domain&) = delete;
  hazard_domain& operator=(const hazard_domain&) = delete;
  hazard_domain(hazard_domain&&) = delete;
  hazard_domain& operator=(hazard_domain&&) = delete;

  ~hazard_domain() {
    std::lock_guard lock(mu_);
    for (auto& e : retired_) {
      e.deleter(e.ptr);
    }
    retired_.clear();
    for (auto* rec : active_) {
      if (detail::tls_record == rec) {
        detail::tls_record = nullptr;
      }
      for (auto& e : rec->retired_) {
        e.deleter(e.ptr);
      }
      delete rec;
    }
    active_.clear();
    for (auto* rec : zombies_) {
      if (detail::tls_record == rec) {
        detail::tls_record = nullptr;
      }
      for (auto& e : rec->retired_) {
        e.deleter(e.ptr);
      }
      delete rec;
    }
    zombies_.clear();
  }

  hazard_record& register_thread() {
    if (detail::tls_record == nullptr || detail::tls_record->domain_ != this) {
      auto* rec = new hazard_record;
      rec->domain_ = this;
      detail::tls_record = rec;
      std::lock_guard lock(mu_);
      active_.push_back(rec);
    }
    return *detail::tls_record;
  }

  void deregister_thread() {
    auto* rec = detail::tls_record;
    if (rec == nullptr || rec->domain_ != this) {
      return;
    }
    std::lock_guard lock(mu_);
    active_.erase(
        std::remove(active_.begin(), active_.end(), rec),
        active_.end());
    for (auto& e : rec->retired_) {
      retired_.push_back(e);
    }
    rec->retired_.clear();
    rec->domain_ = nullptr;
    zombies_.push_back(rec);
    detail::tls_record = nullptr;
  }

  void retire(void* ptr, void (*deleter)(void*) noexcept) {
    std::lock_guard lock(mu_);
    retired_.push_back({ptr, deleter});
    const std::size_t threshold =
        std::max<std::size_t>(64, 2 * active_.size() * hazard_record::kSlots);
    if (retired_.size() >= threshold) {
      scan_locked();
    }
  }

  void scan() {
    std::lock_guard lock(mu_);
    scan_locked();
  }

  [[nodiscard]] std::size_t thread_count() {
    std::lock_guard lock(mu_);
    return active_.size();
  }

 private:
  void scan_locked() {
    if (retired_.empty()) {
      return;
    }
    std::atomic_thread_fence(std::memory_order_seq_cst);
    std::unordered_set<void*> hazards;
    for (auto* rec : active_) {
      for (int s = 0; s < hazard_record::kSlots; ++s) {
        void* val = rec->slots_[s].load(std::memory_order_acquire);
        if (val != nullptr) {
          hazards.insert(val);
        }
      }
    }
    std::size_t j = 0;
    for (std::size_t i = 0; i < retired_.size(); ++i) {
      if (hazards.count(retired_[i].ptr)) {
        retired_[j++] = retired_[i];
      } else {
        retired_[i].deleter(retired_[i].ptr);
      }
    }
    retired_.resize(j);
  }

  std::mutex mu_;
  std::vector<hazard_record*> active_;
  std::vector<hazard_record*> zombies_;
  std::vector<hp_retired_entry> retired_;
};

// ---------------------------------------------------------------------------
// mpsc_queue<T> — multiple-producer, single-consumer linked queue over heap
// nodes, using hazard pointers for safe memory reclamation.
//
// Producers allocate a node per push, and reclamation runs scans under the
// domain's mutex, so progress depends on allocation success and scan work:
// this implementation makes no formal lock-free or wait-free claim.
//
// This is the retained implementation name; norn/queue/mpsc_linked_queue.hpp
// aliases it as the canonical 0.2 name until the header is decomposed in 0.3.
// ---------------------------------------------------------------------------

template <typename T>
class mpsc_queue {
  static_assert(std::is_nothrow_move_constructible_v<T>,
                "mpsc_queue requires nothrow move-constructible T");
  static_assert(std::is_nothrow_destructible_v<T>,
                "mpsc_queue requires nothrow destructible T");

  struct node {
    alignas(T) std::byte storage[sizeof(T)];
    std::atomic<void*> next{nullptr};
    bool has_value = false;  // false for the sentinel, true for payload nodes
  };

 public:
  explicit mpsc_queue(hazard_domain& domain) : domain_(domain) {
    sentinel_ = new node;
    sentinel_->has_value = false;
    head_.store(sentinel_, std::memory_order_relaxed);
    tail_.store(sentinel_, std::memory_order_relaxed);
  }

  mpsc_queue(const mpsc_queue&) = delete;
  mpsc_queue& operator=(const mpsc_queue&) = delete;

  ~mpsc_queue() {
    // Walk the list and destroy every node.
    // The sentinel's storage was never T-constructed, so skip its destruction.
    node* current = static_cast<node*>(head_.load(std::memory_order_relaxed));
    while (current != nullptr) {
      node* next = static_cast<node*>(current->next.load(std::memory_order_relaxed));
      if (current->has_value) {
        std::launder(reinterpret_cast<T*>(current->storage))->~T();
      }
      delete current;
      current = next;
    }
  }

  template <typename... Args>
  bool try_push(Args&&... args) {
    auto* rec = &domain_.register_thread();
    node* new_node = new node;
    new_node->has_value = true;
    ::new (static_cast<void*>(new_node->storage)) T(std::forward<Args>(args)...);

    for (;;) {
      hazard_ptr<node, 0> tail_guard(*rec, tail_);
      node* t = tail_guard.get();
      void* next_raw = t->next.load(std::memory_order_acquire);

      void* tail_expected = t;
      if (next_raw != nullptr) {
        node* next = static_cast<node*>(next_raw);
        tail_.compare_exchange_strong(
            tail_expected, next,
            std::memory_order_release, std::memory_order_relaxed);
        continue;
      }

      if (t->next.compare_exchange_strong(
              next_raw, static_cast<void*>(new_node),
              std::memory_order_release, std::memory_order_relaxed)) {
        void* tail_expected2 = t;
        tail_.compare_exchange_strong(
            tail_expected2, static_cast<void*>(new_node),
            std::memory_order_release, std::memory_order_relaxed);
        return true;
      }
    }
  }

  std::optional<T> try_pop() {
    auto* rec = &domain_.register_thread();

    for (;;) {
      hazard_ptr<node, 0> head_guard(*rec, head_);
      node* h = head_guard.get();
      hazard_ptr<node, 1> next_guard(*rec, h->next);
      node* next_val = next_guard.get();

      if (h != static_cast<node*>(head_.load(std::memory_order_acquire))) {
        continue;
      }

      if (next_val == nullptr) {
        return std::nullopt;
      }

      // If head == tail but next is non-null, tail is lagging.  Help
      // advance tail before attempting to pop; retiring the head while
      // tail still points to it would allow a scanner to free the node
      // that a concurrent producer is about to dereference.
      node* t = static_cast<node*>(tail_.load(std::memory_order_acquire));
      if (h == t) {
        void* expected = t;
        tail_.compare_exchange_strong(
            expected, next_val, std::memory_order_release,
            std::memory_order_relaxed);
        continue;
      }

      // Head and tail differ: safe to pop.
      void* expected = h;
      if (head_.compare_exchange_strong(
              expected, static_cast<void*>(next_val),
              std::memory_order_release, std::memory_order_relaxed)) {
        T value = std::move(
            *std::launder(reinterpret_cast<T*>(next_val->storage)));
        if (h->has_value) {
          domain_.retire(h, delete_node);
        } else {
          domain_.retire(h, delete_sentinel);
        }
        return value;
      }
    }
  }

  [[nodiscard]] bool empty() const noexcept {
    node* h = static_cast<node*>(head_.load(std::memory_order_acquire));
    return h->next.load(std::memory_order_acquire) == nullptr;
  }

 private:
  static void delete_sentinel(void* p) noexcept {
    delete static_cast<node*>(p);
  }

  static void delete_node(void* p) noexcept {
    node* n = static_cast<node*>(p);
    std::launder(reinterpret_cast<T*>(n->storage))->~T();
    delete n;
  }

  hazard_domain& domain_;
  node* sentinel_ = nullptr;
  std::atomic<void*> head_{nullptr};
  std::atomic<void*> tail_{nullptr};
};

}  // namespace norn