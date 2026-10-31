package com.trollm.app

/** Monotonic renewal cadence; the service owns acquisition/release on Main. */
internal class RunWakeLease {
    private var lastAcquiredMs: Long? = null

    fun acquired(nowMs: Long) {
        lastAcquiredMs = nowMs
    }

    // Null represents a failed observation, false a paused/terminal/inactive run.
    fun shouldRenew(nowMs: Long, observeInBackground: Boolean?): Boolean {
        val previous = lastAcquiredMs ?: return false
        return observeInBackground == true && nowMs - previous >= RENEW_INTERVAL_MS
    }

    fun reset() {
        lastAcquiredMs = null
    }

    companion object {
        const val TIMEOUT_MS = 10 * 60 * 1000L
        const val RENEW_INTERVAL_MS = 5 * 60 * 1000L
    }
}
