package com.trollm.app

import java.net.URI
import org.json.JSONObject

/** Setup defaults are separate from the v1 execution graph. */
data class ModelRequirement(
    val label: String,
    val url: String,
    val sha256: String,
    val sizeBytes: Long,
    val ctx: Int,
    val temperature: Double = 0.2,
    val topP: Double = 0.95,
) {
    fun validate() {
        require(label.isNotBlank()) { "Model label is missing." }
        require(Regex("[a-f0-9]{64}").matches(sha256)) { "Invalid model SHA256." }
        require(sizeBytes > 0 && ctx > 0) { "Invalid model size or context." }
        require(temperature.isFinite() && temperature in 0.0..2.0 && topP.isFinite() && topP > 0 && topP <= 1) { "Invalid sampling settings." }
        validateSetupUrl(url)
    }
}

data class ConnectionRequirement(val url: String, val auth: String, val issuerUrl: String?)

data class FlowSetup(
    val title: String,
    val description: String?,
    val inputLabel: String?,
    val inputPlaceholder: String?,
    val models: Map<String, ModelRequirement>,
    val connections: Map<String, ConnectionRequirement>,
    val inputFields: List<StructuredInputField> = emptyList(),
) {
    fun configuration(paths: Map<String, String>): String {
        require(paths.keys == models.keys && paths.values.all { it.isNotBlank() }) { "Models are not ready." }
        val configured = JSONObject()
        models.forEach { (slot, model) ->
            configured.put(slot, JSONObject().put("path", paths.getValue(slot))
                .put("sha256", model.sha256).put("ctx", model.ctx)
                .put("temperature", model.temperature).put("topP", model.topP))
        }
        val servers = JSONObject()
        connections.forEach { (name, connection) -> servers.put(name, JSONObject().put("url", connection.url)) }
        return JSONObject().put("version", 1).put("models", configured).put("connections", servers).toString()
    }

    companion object {
        fun parse(json: String): FlowSetup {
            val root = JSONObject(json)
            require(root.getString("format") == "trollm.flow-setup.v1") { "Unsupported setup format." }
            val models = linkedMapOf<String, ModelRequirement>()
            val entries = root.getJSONObject("models")
            entries.keys().forEach { slot ->
                requireSafeSetupKey(slot)
                val item = entries.getJSONObject(slot)
                models[slot] = ModelRequirement(item.getString("label"), item.getString("url"),
                    item.getString("sha256"), item.getLong("sizeBytes"), item.getInt("ctx"),
                    item.optDouble("temperature", 0.2), item.optDouble("topP", 0.95)).also { it.validate() }
            }
            val connections = linkedMapOf<String, ConnectionRequirement>()
            root.optJSONObject("connections")?.let { servers ->
                servers.keys().forEach { name ->
                    requireSafeSetupKey(name)
                    val item = servers.getJSONObject(name)
                    val url = item.getString("url").also(::validateSetupUrl)
                    val auth = item.getString("auth")
                    require(auth == "bearer" || auth == "oauth") { "Unsupported connection authentication." }
                    val issuer = item.optString("issuerUrl").takeIf { it.isNotBlank() }?.also(::validateSetupUrl)
                    connections[name] = ConnectionRequirement(url, auth, issuer)
                }
            }
            val title = root.getString("title")
            require(title.isNotBlank()) { "Setup title is missing." }
            val input = root.optJSONObject("input")
            return FlowSetup(title, root.optString("description").takeIf { it.isNotBlank() },
                input?.optString("label")?.takeIf { it.isNotBlank() },
                input?.takeUnless { it.isNull("placeholder") }?.optString("placeholder")?.takeIf { it.isNotBlank() }, models, connections,
                StructuredInput.fields(input))
        }
    }
}

internal fun requireSafeSetupKey(value: String) {
    require(Regex("[A-Za-z0-9][A-Za-z0-9_-]*").matches(value)) { "Invalid setup slot or connection name." }
}

internal fun validateSetupUrl(value: String) {
    val uri = URI(value)
    require(uri.scheme == "https" && !uri.host.isNullOrBlank() && uri.rawUserInfo == null && uri.rawFragment == null) { "Use an HTTPS URL without embedded credentials or a fragment." }
}
