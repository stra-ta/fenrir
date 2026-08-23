#pragma once

#include <atomic>
#include <cstddef>

#include "norn/core/cache_line.hpp"

namespace norn {

template <class T>
struct alignas(cache_line_size) cache_aligned {
  T value;
};

}  // namespace norn
