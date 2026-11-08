package com.trollm.app

import android.content.Context
import org.json.JSONObject
import java.io.File
import uniffi.trollm_uniffi.TrollmRuntime

/** Provisioned source machine identity + existing importer/model/MCP lifecycle. */
object MoraySource {
    fun client(context: Context, profile: MorayProfile): MorayPortable {
        val provider = MorayMachineInstallation.provider(context, profile)
        val grant = provider.pin()
        return MorayPortable(profile, File(context.filesDir, "trollm/moray/${profile.key}"),
            MorayHttps(context.filesDir),
            { provider.accessToken(grant) }, grant,
            validateOwner = { owner ->
                require(provider.pin() == grant)
                MorayMachineInstallation.requireOwner(context, profile, owner)
                require(provider.pin() == grant)
            },
            validateReporter = { reporter ->
                require(provider.pin() == grant)
                MorayMachineInstallation.requireReporter(context, profile, reporter)
                require(provider.pin() == grant)
            })
    }

    fun install(context: Context, profile: MorayProfile, publicationId: String, runtime: TrollmRuntime): JSONObject =
        client(context, profile).pull(publicationId) { bytes ->
            val result = runtime.installSourceBundle(bytes)
            JSONObject().put("archiveSha256", result.archiveSha256)
                .put("installationSha256", result.installationSha256)
                .put("flowSha256", result.flowSha256).put("flowId", result.flowId)
                .put("flowVersion", result.flowVersion.toLong())
        }
    /** Fresh starts check source entitlement; existing cold resume and report
     * retries remain separate from installation/download and never start again. */
    fun authorizeFreshStart(context: Context, association: JSONObject) {
        val profile = profileFromAssociation(association).copy(consumerRevision = BuildConfig.SOURCE_REVISION)
        require(association.sourceString("profile") == profile.key)
        val provider = MorayMachineInstallation.provider(context, profile)
        require(provider.pin() == association.sourceString("grantId"))
        val original = association.getJSONObject("descriptor")
        require(JsonValues.same(client(context, profile).descriptor(original.sourceString("publicationId")), original))
        MorayMachineInstallation.requireOwner(context, profile, original.getJSONObject("source").getJSONObject("owner"))
        // A removal/replacement while the HTTPS read was pending cannot retain
        // the original publication authority merely because the response arrived.
        require(provider.pin() == association.sourceString("grantId"))
    }

    fun profileFromAssociation(association: JSONObject): MorayProfile {
        val value = association.getJSONObject("profileConfig")
        val profile = MorayProfile(value.sourceString("api"), value.sourceString("issuer"), value.sourceString("clientId"), value.sourceString("consumerRevision"))
        require(profile.key == association.sourceString("profile"))
        return profile
    }

    fun reports(context: Context, runtime: TrollmRuntime): MorayRunReports {
        val device = MorayMachineInstallation.deviceId(context)
        return MorayRunReports(File(context.filesDir, "trollm/moray-report-refs"),
            prepare = { run -> JSONObject(runtime.prepareSourceReport(run, device)) },
            retainNative = { run, native -> retainRunReport(context, profileFromAssociation(native.getJSONObject("source")),
                runtime, run, native.getJSONObject("report")) },
            deliver = { native, report -> client(context, profileFromAssociation(native.getJSONObject("source"))).sendReport(report) })
    }

    /** Exact native run/attempt + original install; observations are supplied by
     * the supported flow adapter and never synthesized from final model prose. */
    fun retainRunReport(context: Context, profile: MorayProfile, runtime: TrollmRuntime,
        runId: String, report: JSONObject): String {
        val association = JSONObject(runtime.sourceRunAssociation(runId))
        require(report.sourceString("runId") == association.sourceString("nativeRunId") &&
            report.sourceString("attemptId") == association.sourceString("nativeAttemptId"))
        // Retain the full original association, including source owner and the
        // dependency-complete native revision, behind this report reference.
        return client(context, profile).retainReport(association, report)
    }

}
