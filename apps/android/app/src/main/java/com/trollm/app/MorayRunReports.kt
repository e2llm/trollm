package com.trollm.app

import org.json.JSONObject
import java.io.File

/** UI-independent report workflow. Retrying delivery cannot call prepare/run. */
class MorayRunReports(
    private val directory: File,
    private val prepare: (String) -> JSONObject,
    private val retainNative: (String, JSONObject) -> String,
    private val deliver: (JSONObject, String) -> JSONObject,
) {
    init { directory.mkdirs(); require(directory.isDirectory) }
    private fun file(runId: String): File {
        require(RunIdentity.valid(runId))
        return File(directory, "$runId.json")
    }
    fun retained(runId: String): JSONObject? = file(runId).takeIf { it.isFile }?.let {
        val saved = JSONObject(it.readText())
        val native = saved.getJSONObject("native")
        require(native.getJSONObject("report").sourceString("runId") == runId &&
            native.getJSONObject("source").sourceString("nativeRunId") == runId &&
            native.getJSONObject("report").sourceString("reportId") == saved.sourceString("reportId"))
        saved
    }
    fun retain(runId: String): JSONObject {
        val native = prepare(runId)
        val reportId = retainNative(runId, native)
        require(native.getJSONObject("report").sourceString("reportId") == reportId)
        val saved = JSONObject().put("reportId", reportId).put("native", native)
        val original = retained(runId)
        if (original?.sourceString("reportId") == reportId) return original
        MorayPortable.atomic(file(runId), saved.toString().toByteArray())
        return saved
    }
    fun send(runId: String): JSONObject {
        val saved = retained(runId) ?: throw MorayFailure("RETAIN_REPORT_FIRST")
        if (saved.has("receipt")) return saved
        val receipt = deliver(saved.getJSONObject("native"), saved.sourceString("reportId"))
        saved.put("receipt", receipt)
        MorayPortable.atomic(file(runId), saved.toString().toByteArray())
        return saved
    }
}
