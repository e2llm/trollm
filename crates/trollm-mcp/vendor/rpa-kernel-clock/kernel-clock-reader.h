#ifndef MORAY_KERNEL_CLOCK_READER_H
#define MORAY_KERNEL_CLOCK_READER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* v1 uses fixed-width fields. Errors are positive errno values, never policy
 * verdicts. No timestamp, validity interval or clock-rate bound is supplied. */
typedef struct moray_kernel_clock_observation_v1 {
    int32_t state;
    int32_t status;
    int32_t synchronized;
    int32_t syscall_error;
    int64_t max_error_us;
    int64_t estimated_error_us;
} moray_kernel_clock_observation_v1;

/* Returns zero after a successful read, otherwise positive errno. On syscall
 * failure out is cleared with state=-1 and syscall_error set. NULL returns
 * EINVAL. Compile kernel-clock-probe.c with MORAY_KERNEL_CLOCK_NO_MAIN for
 * in-process linkage. The caller owns sampling brackets and policy checks. */
int moray_read_kernel_clock_v1(moray_kernel_clock_observation_v1 *out);

#ifdef __cplusplus
}
#endif
#endif
