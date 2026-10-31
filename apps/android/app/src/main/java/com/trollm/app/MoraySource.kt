package com.trollm.app

import android.content.Context
import org.json.JSONObject
import java.io.File
import uniffi.trollm_uniffi.TrollmRuntime

/** Existing AppAuth + native importer/model/MCP lifecycle, not a second runtime. */
object MoraySource {
    fun client(context: Context, profile: MorayProfile): MorayPortable {
        val oauth = OAuthConnections.moray(context)
        val server = "moray-${profile.key}"
        val grant = oauth.pin(server, profile.api) ?: throw MorayFailure("SOURCE_RECONNECT_REQUIRED")
        return MorayPortable(profile, File(context.filesDir, "trollm/moray/${profile.key}"),
            MorayHttps(context.filesDir),
            { oauth.accessTokenBlocking(server, profile.api, grant.grantId) }, grant.grantId)
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
        val oauth = OAuthConnections.moray(context)
        require(oauth.pin("moray-${profile.key}", profile.api)?.grantId == association.sourceString("grantId"))
        val original = association.getJSONObject("descriptor")
        require(JsonValues.same(client(context, profile).descriptor(original.sourceString("publicationId")), original))
    }

    fun profileFromAssociation(association: JSONObject): MorayProfile {
        val value = association.getJSONObject("profileConfig")
        val profile = MorayProfile(value.sourceString("api"), value.sourceString("issuer"), value.sourceString("clientId"), value.sourceString("consumerRevision"))
        require(profile.key == association.sourceString("profile"))
        return profile
    }

    fun reports(context: Context, runtime: TrollmRuntime): MorayRunReports {
        val prefs = context.getSharedPreferences("moray-source-device", 0)
        val device = synchronized(this) {
            prefs.getString("id", null) ?: java.util.UUID.randomUUID().toString().also {
                check(prefs.edit().putString("id", it).commit())
            }
        }
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
