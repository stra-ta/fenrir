#pragma once

#include <condition_variable>
#include <cstddef>
#include <deque>
#include <mutex>
#include <optional>
#include <utility>

namespace norn {

// A blocking, unbounded reference queue. close() prevents new pushes but lets
// consumers drain values that were already accepted.
template <typename T>
class mutex_queue {
 public:
  mutex_queue() = default;
  mutex_queue(const mutex_queue&) = delete;
  mutex_queue& operator=(const mutex_queue&) = delete;

  bool try_push(const T& value) { return emplace(value); }

  bool try_push(T&& value) { return emplace(std::move(value)); }

  bool push(const T& value) { return try_push(value); }

  bool push(T&& value) { return try_push(std::move(value)); }

  template <typename... Args>
  bool emplace(Args&&... args) {
    {
      std::lock_guard lock(mutex_);
      if (closed_) {
        return false;
      }
      values_.emplace_back(std::forward<Args>(args)...);
    }
    not_empty_.notify_one();
    return true;
  }

  bool try_pop(T& value) {
    std::lock_guard lock(mutex_);
    if (values_.empty()) {
      return false;
    }
    value = std::move(values_.front());
    values_.pop_front();
    return true;
  }

  std::optional<T> pop() {
    std::unique_lock lock(mutex_);
    not_empty_.wait(lock, [this] { return closed_ || !values_.empty(); });
    if (values_.empty()) {
      return std::nullopt;
    }
    T value = std::move(values_.front());
    values_.pop_front();
    return value;
  }

  void close() {
    {
      std::lock_guard lock(mutex_);
      closed_ = true;
    }
    not_empty_.notify_all();
  }

  [[nodiscard]] bool closed() const {
    std::lock_guard lock(mutex_);
    return closed_;
  }

 private:
  mutable std::mutex mutex_;
  std::condition_variable not_empty_;
  std::deque<T> values_;
  bool closed_ = false;
};

// A blocking, bounded reference queue. close() prevents new pushes, wakes
// blocked operations, and preserves already accepted values for draining.
template <typename T, std::size_t Capacity>
class bounded_mutex_queue {
  static_assert(Capacity > 0, "queue capacity must be positive");

 public:
  bounded_mutex_queue() = default;
  bounded_mutex_queue(const bounded_mutex_queue&) = delete;
  bounded_mutex_queue& operator=(const bounded_mutex_queue&) = delete;

  bool try_push(const T& value) { return emplace(value); }

  bool try_push(T&& value) { return emplace(std::move(value)); }

  template <typename... Args>
  bool emplace(Args&&... args) {
    std::lock_guard lock(mutex_);
    if (closed_ || values_.size() == Capacity) {
      return false;
    }
    try {
      values_.emplace_back(std::forward<Args>(args)...);
    } catch (...) {
      not_full_.notify_one();
      throw;
    }
    not_empty_.notify_one();
    return true;
  }

  bool push(const T& value) { return push_impl(value); }

  bool push(T&& value) { return push_impl(std::move(value)); }

  bool try_pop(T& value) {
    std::lock_guard lock(mutex_);
    if (values_.empty()) {
      return false;
    }
    value = std::move(values_.front());
    values_.pop_front();
    not_full_.notify_one();
    return true;
  }

  std::optional<T> pop() {
    std::unique_lock lock(mutex_);
    not_empty_.wait(lock, [this] { return closed_ || !values_.empty(); });
    if (values_.empty()) {
      return std::nullopt;
    }
    T value = std::move(values_.front());
    values_.pop_front();
    not_full_.notify_one();
    return value;
  }

  void close() {
    {
      std::lock_guard lock(mutex_);
      closed_ = true;
    }
    not_empty_.notify_all();
    not_full_.notify_all();
  }

  [[nodiscard]] bool closed() const {
    std::lock_guard lock(mutex_);
    return closed_;
  }

  [[nodiscard]] static constexpr std::size_t capacity() noexcept { return Capacity; }

 private:
  template <typename U>
  bool push_impl(U&& value) {
    std::unique_lock lock(mutex_);
    not_full_.wait(lock, [this] { return closed_ || values_.size() < Capacity; });
    if (closed_) {
      return false;
    }
    try {
      values_.emplace_back(std::forward<U>(value));
    } catch (...) {
      lock.unlock();
      not_full_.notify_one();
      throw;
    }
    lock.unlock();
    not_empty_.notify_one();
    return true;
  }

  mutable std::mutex mutex_;
  std::condition_variable not_empty_;
  std::condition_variable not_full_;
  std::deque<T> values_;
  bool closed_ = false;
};

}  // namespace norn
