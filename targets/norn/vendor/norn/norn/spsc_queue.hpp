#pragma once

// Compatibility header for the Norn 0.1 SPSC queue names. The implementation
// moved to norn/queue/spsc_ring.hpp; every 0.1 name stays available as an
// alias for the canonical type, so existing code compiles unchanged.

#include "norn/queue/spsc_ring.hpp"

namespace norn {

template <typename T, std::size_t Capacity, bool Padded = false, bool SequentiallyConsistent = false>
using spsc_queue = spsc_ring<T, Capacity, Padded, SequentiallyConsistent>;

template <typename T, std::size_t Capacity>
using spsc_queue_padded = spsc_queue<T, Capacity, true>;

// Stronger-order experimental alias. It remains semantically equivalent to the
// acquire/release baseline because seq_cst loads and stores retain the required
// release/acquire synchronization relations.
template <typename T, std::size_t Capacity, bool Padded = false>
using spsc_queue_seq_cst = spsc_queue<T, Capacity, Padded, true>;

}  // namespace norn
