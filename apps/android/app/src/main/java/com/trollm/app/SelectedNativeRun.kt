package com.trollm.app

import android.content.Context
import org.json.JSONObject

/** Internal app callable for a privately provisioned selected installation.
 * No Activity/intent/deep-link entry accepts its credential or installation file.
 * The caller retains this handle until actual native inactive acknowledgement;
 * cancellation or a returned outcome is not an inactive-worker acknowledgement.
 */
internal class SelectedNativeRun private constructor(
    private val context: Context,
    val runId: String,
    private val installationKey: String,
    private val invocationKey: String,
) {
    fun binding(): JSONObject {
        val result = JSONObject(RuntimeHolder.get(context).selectedRunBinding(runId))
        validateBinding(result, runId, installationKey, invocationKey)
        return result
    }

    fun cancel() { RuntimeHolder.get(context).cancel(runId) }

    companion object {
        internal fun validateBinding(result: JSONObject, runId: String,
                                     installationKey: String, invocationKey: String) {
            require(result.getString("schema") == "trollm.selected-worker-binding.v1")
            require(result.getString("nativeRunId") == runId)
            val privateBinding = JSONObject(result.getString("privateBindingJson"))
            require(privateBinding.getString("privateInstallationSha256") == installationKey)
            require(privateBinding.getString("nativeRunId") == runId)
            require(privateBinding.getString("invocationKey") == invocationKey)
            val claim = JSONObject(result.getString("claimJson"))
            require(claim.getJSONObject("identity").getString("invocationKey") == invocationKey)
            require(result.get("workerActive") is Boolean)
            require(claim.getString("nativeRunId") == runId)
            require(JSONObject(result.getString("sourceAssociationJson")).getString("nativeRunId") == runId)
        }

        /** Source entitlement is rechecked by the native installed environment;
         * this wrapper neither performs effects nor creates a second run loop.
         */
        fun start(context: Context, flowSha256: String, installationSha256: String,
                  input: String, configuration: String, association: String,
                  installationKey: String, invocationKey: String,
                  onStage: (String) -> Unit = {}): SelectedNativeRun {
            require(Regex("[a-f0-9]{64}").matches(installationKey))
            require(invocationKey.isNotEmpty() && invocationKey.length <= 256)
            val app = context.applicationContext
            val runtime = RuntimeHolder.get(app)
            check(!runtime.hasActiveRun()) { "Finish or cancel the active run before starting another." }
            onStage("source-authority")
            MoraySource.authorizeFreshStart(app, JSONObject(association))
            onStage("input-admission")
            onStage("input-setup")
            val setup = JSONObject(checkNotNull(runtime.sourceBundleSetup(installationSha256)))
            val fields = StructuredInput.fields(setup.getJSONObject("input"))
            onStage("input-normalization")
            val normalized = StructuredInput.normalizeOriginal(fields, input)
            val admittedAssociation = SourceInputAdmission.attach(association, input, normalized, onStage) { raw, apk ->
                runtime.admitSourceInput(installationSha256, raw, apk)
            }
            onStage("native-start")
            val run = runtime.startSelectedSourceRunConfigured(
                flowSha256, installationSha256, input, configuration, admittedAssociation, installationKey, invocationKey)
            // Never retry start if later binding/readback fails: the returned run
            // may already own a live worker. Preserve its genuine ID for cancel.
            return SelectedNativeRun(app, run, installationKey, invocationKey)
        }
    }
}
