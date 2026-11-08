#include <stdio.h>
#include <sys/timex.h>
#include <errno.h>
#include "kernel-clock-reader.h"

/* Read-only kernel observation: modes remains zero. No clock adjustment,
 * network access, arguments, environment policy or source-signing authority. */
int moray_read_kernel_clock_v1(moray_kernel_clock_observation_v1 *out) {
    if (!out) return EINVAL;
    *out = (moray_kernel_clock_observation_v1){.state = -1};
    struct timex observation = {0};
    const int state = adjtimex(&observation);
    if (state < 0) {
        const int error = errno > 0 ? errno : EIO;
        out->syscall_error = error;
        return error;
    }
    out->state = state;
    out->status = observation.status;
    out->synchronized = state == TIME_OK &&
        !(observation.status & (STA_UNSYNC | STA_CLOCKERR));
    out->max_error_us = observation.maxerror;
    out->estimated_error_us = observation.esterror;
    return 0;
}

#ifndef MORAY_KERNEL_CLOCK_NO_MAIN
int main(int argc, char **argv) {
    (void)argv;
    if (argc != 1) {
        fputs("Usage: kernel-clock-probe (read-only, no arguments)\n", stderr);
        return 64;
    }
    moray_kernel_clock_observation_v1 observation;
    if (moray_read_kernel_clock_v1(&observation) != 0) {
        fputs("Kernel clock observation unavailable\n", stderr);
        return 1;
    }
    printf("{\"schemaVersion\":\"moray.kernel-clock-observation.v1\","
           "\"state\":%d,\"synchronized\":%s,\"maxErrorUs\":%ld,\"estimatedErrorUs\":%ld}\n",
           (int)observation.state, observation.synchronized ? "true" : "false",
           (long)observation.max_error_us, (long)observation.estimated_error_us);
    return 0;
}
#endif
