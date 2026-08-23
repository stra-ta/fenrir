#pragma once

#include <atomic>

#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
#include <immintrin.h>
#elif (defined(_M_ARM64) || defined(_M_ARM)) && defined(_MSC_VER)
#include <intrin.h>
#endif

namespace norn {

inline void cpu_relax() noexcept {
#if defined(__x86_64__) || defined(_M_X64) || defined(__i386__) || defined(_M_IX86)
  _mm_pause();
#elif (defined(__aarch64__) || defined(__arm__)) && (defined(__GNUC__) || defined(__clang__))
  __asm__ __volatile__("yield");
#elif (defined(_M_ARM64) || defined(_M_ARM)) && defined(_MSC_VER)
  __yield();
#else
  std::atomic_signal_fence(std::memory_order_seq_cst);
#endif
}

}  // namespace norn
