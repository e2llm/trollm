package com.trollm.app.ui

import org.json.JSONObject
import uniffi.trollm_uniffi.EventLine

/**
 * Render one event-log line as a progress string, keyed ONLY off the §7
 * closed set of event types and their structured fields. The app never
 * parses model prose — prose never appears in the event log (§10).
 */
fun renderEvent(event: EventLine): String {
    val fields = runCatching { JSONObject(event.json) }.getOrNull()
    fun str(key: String) = fields?.optString(key)?.takeIf { it.isNotEmpty() }
    fun num(key: String) = fields?.optLong(key, -1)?.takeIf { it >= 0 }
    return when (event.eventType) {
        "run_started" -> "Run started — plan ${str("plan") ?: "?"}"
        "plan_loaded" -> "Plan loaded: ${str("plan") ?: "?"}"
        "node_entered" -> "→ ${str("node") ?: "?"}"
        "node_exited" -> "← ${str("node") ?: "?"}"
        "model_call" -> "model[${str("slot") ?: "?"}] " +
            "${num("tokensIn") ?: 0}→${num("tokensOut") ?: 0} tokens, ${num("ms") ?: 0} ms"
        "tool_call_intent" -> "tool intent: ${str("server")}.${str("tool")}"
        "tool_call_result" -> "tool result: ${str("server")}.${str("tool")}" +
            (str("provenance")?.let { " ($it)" } ?: "")
        "cache_hit" -> "cache hit: ${str("server")}.${str("tool")}"
        "served_from_file" -> "served from file: ${str("server")}.${str("tool")}"
        "artifact_written" -> "artifact written (${num("bytes") ?: 0} bytes)"
        "fetch_progress" -> "fetching pages… ${num("pages") ?: "?"} pages, ${num("items") ?: "?"} items"
        "transition" -> "${str("from")} —[${str("key")}]→ ${str("to")}"
        "budget_exhausted" -> "budget exhausted: ${str("budget")}"
        "paused" -> "paused: ${str("reason") ?: ""}"
        "resumed" -> "resumed"
        "error" -> "error[${str("class") ?: "?"}]: ${str("detail") ?: ""}"
        "run_ended" -> "run ended: ${str("outcome") ?: "?"}"
        else -> "${event.eventType} (#${event.seq})"
    }
}
