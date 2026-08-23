#pragma once

// Canonical 0.2 name for the hazard-pointer-based MPSC linked queue. The
// implementation still lives in norn/hazard_pointer.hpp; its decomposition
// out of that header is planned for 0.3, so this is an alias for now.

#include "norn/hazard_pointer.hpp"

namespace norn {

template <typename T>
using mpsc_linked_queue = mpsc_queue<T>;

}  // namespace norn
