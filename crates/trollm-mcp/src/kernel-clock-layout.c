#include <stddef.h>
#include "kernel-clock-reader.h"

/* Compile with the actual target compiler as well as Rust's repr(C) checks. */
_Static_assert(sizeof(moray_kernel_clock_observation_v1) == 32, "kernel clock ABI size");
_Static_assert(offsetof(moray_kernel_clock_observation_v1, state) == 0, "state offset");
_Static_assert(offsetof(moray_kernel_clock_observation_v1, status) == 4, "status offset");
_Static_assert(offsetof(moray_kernel_clock_observation_v1, synchronized) == 8, "sync offset");
_Static_assert(offsetof(moray_kernel_clock_observation_v1, syscall_error) == 12, "error offset");
_Static_assert(offsetof(moray_kernel_clock_observation_v1, max_error_us) == 16, "max error offset");
_Static_assert(offsetof(moray_kernel_clock_observation_v1, estimated_error_us) == 24, "estimated error offset");
