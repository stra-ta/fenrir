#pragma once

// Compatibility header for the Norn 0.1 MPMC queue name. The implementation
// moved to norn/queue/mpmc_ring.hpp; the 0.1 name stays available as an alias
// for the canonical type, so existing code compiles unchanged.

#include "norn/queue/mpmc_ring.hpp"

namespace norn {

template <typename T, std::size_t Capacity>
using mpmc_queue = mpmc_ring<T, Capacity>;

}  // namespace norn
