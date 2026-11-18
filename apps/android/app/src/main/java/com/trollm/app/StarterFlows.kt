package com.trollm.app

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.TrollmRuntime

/** Install current packaged starters idempotently; retain imports and past runs. */
object StarterFlows {
    private val lock = Mutex()
    private val catalog = StarterFlowCatalog()
    private val assets = mapOf(
        BundledStarter.TEXT_SUMMARY to "flows/text-summary.tar",
        BundledStarter.CHECK_TOTAL to "flows/check-total.tar",
        BundledStarter.BROWSER_SUMMARY to "flows/browser-summary.tar",
    )

    fun verifiedKind(flowSha: String): BundledStarter? = catalog.verifiedKind(flowSha)

    /** Cold routes can resolve stock identity through the same bounded local admission. */
    suspend fun resolveKind(context: Context, runtime: TrollmRuntime, flowSha: String): BundledStarter? =
        withContext(Dispatchers.IO) {
            lock.withLock {
                catalog.resolveKind(flowSha) { installAsset(context, runtime, it) }
            }
        }

    internal suspend fun ensureInstalled(context: Context, runtime: TrollmRuntime): StarterInstallation =
        withContext(Dispatchers.IO) {
            lock.withLock {
                catalog.install { installAsset(context, runtime, it) }
            }
        }

    private fun installAsset(context: Context, runtime: TrollmRuntime, kind: BundledStarter): FlowInfo {
        val path = assets.getValue(kind)
        val bytes = context.applicationContext.assets.open(path).use { BundleInput.read(it) }
        // An existing installation must match the complete manifest, including
        // prompts, registry and setup, for its identity to be admitted again.
        return runtime.installFlowBundle(bytes).installedFlow()
    }
}

internal data class StarterInstallation(
    val verified: Map<FlowInfo, BundledStarter>,
    val errors: List<UiError>,
) {
    fun kind(flowSha: String): BundledStarter? = verified.entries.firstOrNull { it.key.sha256 == flowSha }?.value
}

/** Updates are serialized by the caller. Warm reads are used without waiting for I/O. */
internal class StarterFlowCatalog {
    @Volatile
    private var latest: StarterInstallation? = null

    fun verifiedKind(flowSha: String): BundledStarter? = latest?.kind(flowSha)

    fun resolveKind(flowSha: String, installAsset: (BundledStarter) -> FlowInfo): BundledStarter? =
        (latest ?: install(installAsset)).kind(flowSha)

    fun install(installAsset: (BundledStarter) -> FlowInfo): StarterInstallation {
        val verified = mutableMapOf<FlowInfo, BundledStarter>()
        val errors = mutableListOf<UiError>()
        for (kind in BundledStarter.entries) {
            catchingOperation { installAsset(kind) }.onSuccess { flow ->
                verified[flow] = kind
            }.onFailure { failure ->
                errors += operationError(UiOperation.INSTALL_STARTERS, failure)
            }
        }
        return StarterInstallation(verified, errors).also { latest = it }
    }
}
