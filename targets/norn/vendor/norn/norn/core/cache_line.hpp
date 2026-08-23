#pragma once

#include <cstddef>
#include <new>
#include <version>

namespace norn {

#if defined(__cpp_lib_hardware_interference_size)
#if defined(__GNUC__) && !defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winterference-size"
#endif
inline constexpr std::size_t cache_line_size = std::hardware_destructive_interference_size;
#if defined(__GNUC__) && !defined(__clang__)
#pragma GCC diagnostic pop
#endif
#else
inline constexpr std::size_t cache_line_size = 64;
#endif

}  // namespace norn
