package com.trollm.app.ui

import org.json.JSONObject
import uniffi.trollm_uniffi.EventLine

/**
 * Render one event-log line as a progress string, keyed ONLY off the §7
 * closed set of event types and their structured fields. The app never
 * parses model prose — prose never appears in the event log (§10).
 */
fun renderEvent(event: EventLine): String = renderEventJson(event.eventType, event.seq, event.json)

internal fun renderEventJson(eventType: String, seq: ULong, json: String): String {
    val fields = runCatching { JSONObject(json) }.getOrNull()
    fun str(key: String) = fields?.optString(key)?.takeIf { it.isNotEmpty() }
    // JSONObject.optLong coerces strings and truncates fractions. Only recorded,
    // exact nonnegative integers in range are measurements we can display.
    fun num(key: String): Long? {
        val value = fields?.opt(key) as? Number ?: return null
        val decimal = value.toString().toBigDecimalOrNull() ?: return null
        return runCatching { decimal.longValueExact() }.getOrNull()?.takeIf { it >= 0 }
    }
    fun measured(key: String) = num(key)?.toString() ?: "unknown"
    val finishReason = (fields?.opt("finishReason") as? String)?.takeIf { it.isNotBlank() }
    return when (eventType) {
        "run_started" -> "Run started — plan ${str("plan") ?: "?"}"
        "plan_loaded" -> "Plan loaded: ${str("plan") ?: "?"}"
        "node_entered" -> "→ ${str("node") ?: "?"}"
        "node_exited" -> if (str("phase") == "attempt") {
            "Timing recorded: ${str("node") ?: "?"}"
        } else "← ${str("node") ?: "?"}"
        "model_call" -> "model[${str("slot") ?: "?"}] " +
            "${measured("tokensIn")}→${measured("tokensOut")} tokens, ${measured("ms")} ms" +
            (finishReason?.let { "; finish $it" } ?: "")
        "tool_call_intent" -> "tool intent: ${str("server")}.${str("tool")}"
        "tool_call_result" -> "tool result: ${str("server")}.${str("tool")}" +
            (str("provenance")?.let { " ($it)" } ?: "")
        "cache_hit" -> "cache hit: ${str("server")}.${str("tool")}"
        "served_from_file" -> "served from file: ${str("server")}.${str("tool")}"
        "artifact_written" -> "artifact written (${measured("bytes")} bytes)"
        "fetch_progress" -> "fetching pages… ${num("pages") ?: "?"} pages, ${num("items") ?: "?"} items"
        "transition" -> "${str("from")} —[${str("key")}]→ ${str("to")}"
        "budget_exhausted" -> "budget exhausted: ${str("budget")}"
        "paused" -> "paused: ${str("reason") ?: ""}"
        "resumed" -> "resumed"
        "error" -> "error[${str("class") ?: "?"}]: ${str("detail") ?: ""}"
        "run_ended" -> "run ended: ${str("outcome") ?: "?"}"
        else -> "$eventType (#$seq)"
    }
}
