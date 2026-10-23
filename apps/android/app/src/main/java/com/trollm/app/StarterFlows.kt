package com.trollm.app

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.TrollmRuntime

/** Install current packaged starters idempotently; retain imports and past runs. */
object StarterFlows {
    private val lock = Mutex()
    private val assets = listOf("flows/text-summary.tar", "flows/browser-summary.tar", "flows/check-total.tar")

    suspend fun ensureInstalled(context: Context, runtime: TrollmRuntime) = withContext(Dispatchers.IO) {
        lock.withLock {
            for (path in assets) {
                val bytes = context.applicationContext.assets.open(path).use { it.readBytes() }
                runtime.installFlowBundle(bytes)
            }
        }
    }
}
