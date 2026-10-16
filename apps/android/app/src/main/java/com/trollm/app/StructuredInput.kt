package com.trollm.app

import java.net.URI
import org.json.JSONObject

data class StructuredInputField(
    val key: String,
    val label: String,
    val placeholder: String? = null,
    val required: Boolean = false,
    val kind: String = "text",
    val maxLength: Int = 4096,
)

/** The FFI remains a string: only declared fields enter its JSON object. */
object StructuredInput {
    const val MAX_JSON_BYTES = 65536

    fun fields(input: JSONObject?): List<StructuredInputField> {
        if (input == null || !input.has("fields")) return emptyList()
        val entries = input.getJSONArray("fields")
        require(entries.length() in 1..8) { "Input must declare between one and eight fields." }
        val fields = (0 until entries.length()).map { index ->
            val item = entries.getJSONObject(index)
            val required = if (item.has("required")) item.get("required") else false
            require(required is Boolean) { "Input required must be true or false." }
            val maximum = if (item.has("maxLength")) item.get("maxLength") else 4096
            require(maximum is Int || maximum is Long) { "Input maxLength must be a whole number." }
            val limit = (maximum as Number).toLong()
            require(limit in 1..16384) { "Input maxLength must be between 1 and 16384." }
            StructuredInputField(text(item, "key"), text(item, "label"),
                if (item.has("placeholder") && !item.isNull("placeholder")) text(item, "placeholder") else null,
                required, if (item.has("kind")) text(item, "kind") else "text", limit.toInt())
        }
        validateFields(fields)
        return fields
    }

    private fun text(item: JSONObject, name: String): String = item.get(name) as? String
        ?: throw IllegalArgumentException("Input metadata requires text values.")

    private fun validateFields(fields: List<StructuredInputField>) {
        require(fields.size in 1..8 && fields.map { it.key }.toSet().size == fields.size) { "Input field keys must be unique." }
        fields.forEach {
            require(Regex("[a-zA-Z_][a-zA-Z0-9_]{0,31}").matches(it.key)) { "Input field key is invalid." }
            require(it.label.isNotBlank()) { "Input field label is missing." }
            require(it.kind == "text" || it.kind == "url") { "Input field kind is unsupported." }
            require(it.maxLength in 1..16384) { "Input field length limit is invalid." }
        }
    }

    fun errors(fields: List<StructuredInputField>, values: Map<String, String>): Map<String, String> {
        if (fields.isEmpty()) return emptyMap()
        validateFields(fields)
        val errors = linkedMapOf<String, String>()
        fields.forEach { field ->
            val value = values[field.key].orEmpty().trim()
            val message = when {
                field.required && value.isBlank() -> "${field.label} is required."
                value.codePointCount(0, value.length) > field.maxLength -> "${field.label} must be at most ${field.maxLength} characters."
                value.isNotEmpty() && field.kind == "url" && !validUrl(value) -> "${field.label} must be an HTTP or HTTPS URL with a host and no embedded credentials."
                else -> null
            }
            if (message != null) errors[field.key] = message
        }
        return errors
    }

    fun serialize(fields: List<StructuredInputField>, values: Map<String, String>, legacyText: String): String {
        if (fields.isEmpty()) return legacyText
        require(errors(fields, values).isEmpty()) { "Check the input fields before starting." }
        val json = JSONObject()
        fields.forEach { json.put(it.key, values[it.key].orEmpty().trim()) }
        val serialized = json.toString()
        require(serialized.toByteArray(Charsets.UTF_8).size <= MAX_JSON_BYTES) {
            "Combined input is too large. Shorten the text and try again."
        }
        return serialized
    }

    private fun validUrl(value: String): Boolean = try {
        val uri = URI(value)
        uri.scheme?.lowercase() in setOf("http", "https") && !uri.host.isNullOrBlank() &&
            uri.rawUserInfo == null && uri.port in -1..65535
    } catch (_: Exception) { false }
}
