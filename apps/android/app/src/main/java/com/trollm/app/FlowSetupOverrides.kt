package com.trollm.app

import org.json.JSONObject

fun FlowSetup.modelDisplayLabel(slot: String, defaults: FlowSetup): String =
    if (models.getValue(slot).sha256 != defaults.models.getValue(slot).sha256) "$slot (custom model)"
    else defaults.models.getValue(slot).label

/** Only explicit differences from the pinned descriptor are persisted. */
data class ModelOverride(
    val url: String,
    val sha256: String,
    val sizeBytes: Long,
    val ctx: Int,
    val temperature: Double,
    val topP: Double,
) {
    fun applyTo(default: ModelRequirement): ModelRequirement = default.copy(
        url = url, sha256 = sha256, sizeBytes = sizeBytes, ctx = ctx,
        temperature = temperature, topP = topP,
    ).also { it.validate() }
}

data class FlowSetupOverrides(
    val models: Map<String, ModelOverride> = emptyMap(),
    val connections: Map<String, String> = emptyMap(),
) {
    fun applyTo(defaults: FlowSetup): FlowSetup {
        require(models.keys.all { it in defaults.models } && connections.keys.all { it in defaults.connections }) {
            "Saved settings refer to a model or connection no longer declared by this flow. Restore defaults."
        }
        return defaults.copy(
            models = defaults.models.mapValues { (slot, model) -> models[slot]?.applyTo(model) ?: model },
            connections = defaults.connections.mapValues { (name, connection) ->
                connections[name]?.let { connection.copy(url = it.also(::validateSetupUrl)) } ?: connection
            },
        )
    }

    fun toJson(): String {
        val modelJson = JSONObject()
        models.forEach { (slot, model) -> modelJson.put(slot, JSONObject()
            .put("url", model.url).put("sha256", model.sha256).put("sizeBytes", model.sizeBytes)
            .put("ctx", model.ctx).put("temperature", model.temperature).put("topP", model.topP)) }
        val connectionJson = JSONObject()
        connections.forEach { (name, url) -> connectionJson.put(name, JSONObject().put("url", url)) }
        return JSONObject().put("version", 1).put("models", modelJson).put("connections", connectionJson).toString()
    }

    companion object {
        fun parse(json: String, defaults: FlowSetup): FlowSetupOverrides {
            val root = JSONObject(json)
            root.onlyKeys(setOf("version", "models", "connections"))
            require(root.integer("version") == 1L) { "Unsupported advanced settings version." }
            val models = linkedMapOf<String, ModelOverride>()
            root.getJSONObject("models").let { entries ->
                entries.keys().forEach { slot ->
                    requireSafeSetupKey(slot)
                    val item = entries.getJSONObject(slot)
                    item.onlyKeys(setOf("url", "sha256", "sizeBytes", "ctx", "temperature", "topP"))
                    val ctx = item.integer("ctx")
                    require(ctx in 1..Int.MAX_VALUE.toLong()) { "Context must be a positive integer." }
                    models[slot] = ModelOverride(item.string("url"), item.string("sha256"),
                        item.integer("sizeBytes"), ctx.toInt(), item.number("temperature"), item.number("topP"))
                }
            }
            val connections = linkedMapOf<String, String>()
            root.getJSONObject("connections").let { entries ->
                entries.keys().forEach { name ->
                    requireSafeSetupKey(name)
                    val item = entries.getJSONObject(name)
                    item.onlyKeys(setOf("url"))
                    connections[name] = item.string("url")
                }
            }
            return FlowSetupOverrides(models, connections).also { it.applyTo(defaults) }
        }

        fun between(defaults: FlowSetup, effective: FlowSetup): FlowSetupOverrides {
            require(defaults.models.keys == effective.models.keys && defaults.connections.keys == effective.connections.keys) {
                "Advanced settings must keep the flow's declared models and connections."
            }
            val models = effective.models.filter { (slot, model) -> model != defaults.models.getValue(slot) }
                .mapValues { (_, model) -> ModelOverride(model.url, model.sha256, model.sizeBytes, model.ctx, model.temperature, model.topP) }
            val connections = effective.connections.filter { (name, connection) -> connection.url != defaults.connections.getValue(name).url }
                .mapValues { (_, connection) -> connection.url }
            return FlowSetupOverrides(models, connections).also { it.applyTo(defaults) }
        }
    }
}

private fun JSONObject.onlyKeys(allowed: Set<String>) {
    require(keys().asSequence().all { it in allowed }) { "Advanced settings contain unsupported fields. Restore defaults." }
}

private fun JSONObject.string(name: String): String = (get(name) as? String)
    ?: throw IllegalArgumentException("Advanced settings contain an invalid text field.")

private fun JSONObject.integer(name: String): Long {
    val value = get(name)
    require(value is Int || value is Long) { "Advanced settings require whole-number size and context values." }
    return (value as Number).toLong()
}

private fun JSONObject.number(name: String): Double = (get(name) as? Number)?.toDouble()
    ?: throw IllegalArgumentException("Advanced settings contain an invalid sampling value.")

/** String persistence is injectable so the real codec/overlay can be tested on JVM. */
class FlowSetupOverrideStore(
    private val read: (String) -> String?,
    private val write: (String, String?) -> Boolean,
) {
    private fun key(flowSha: String): String {
        require(Regex("[a-f0-9]{64}").matches(flowSha)) { "Invalid pinned flow identity." }
        return flowSha
    }

    fun load(flowSha: String, defaults: FlowSetup): FlowSetup =
        read(key(flowSha))?.let { FlowSetupOverrides.parse(it, defaults).applyTo(defaults) } ?: defaults

    fun save(flowSha: String, defaults: FlowSetup, effective: FlowSetup) {
        val overrides = FlowSetupOverrides.between(defaults, effective)
        check(write(key(flowSha), overrides.toJson())) { "Could not save advanced settings." }
    }

    fun restore(flowSha: String) {
        check(write(key(flowSha), null)) { "Could not restore default settings." }
    }
}
