#pragma once

#include <atomic>

#include "norn/core/alignment.hpp"

namespace norn {

template <class T>
using isolated_atomic = cache_aligned<std::atomic<T>>;

}  // namespace norn
