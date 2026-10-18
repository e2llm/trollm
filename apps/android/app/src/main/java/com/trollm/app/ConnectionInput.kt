package com.trollm.app

import java.net.URI
import java.util.Locale

/** Checks manual connection input without retaining or formatting secrets. */
object ConnectionInput {
    fun validServer(server: String): Boolean =
        server.isNotEmpty() && server.none { it.isWhitespace() || it.isISOControl() }

    fun validToken(token: String): Boolean =
        token.isNotEmpty() && token.none { it.isWhitespace() || it.isISOControl() }

    /** Partial edits are input feedback, not connection-storage failures. */
    fun endpointError(value: String): String? = try {
        canonicalEndpoint(value)
        null
    } catch (_: IllegalArgumentException) {
        "Enter a valid HTTPS MCP endpoint without credentials or a fragment."
    }

    /** HTTPS identity: preserve escaped path/query; never collapse distinct resources. */
    fun canonicalEndpoint(value: String): String {
        val uri = try { URI(value) } catch (_: java.net.URISyntaxException) {
            throw IllegalArgumentException("Enter a valid HTTPS MCP endpoint.")
        }
        require(uri.scheme.equals("https", ignoreCase = true) && uri.host != null &&
            uri.rawUserInfo == null && uri.rawFragment == null && uri.port in -1..65535) {
            "Enter an HTTPS MCP endpoint without credentials or a fragment."
        }
        val host = uri.host.lowercase(Locale.ROOT)
        val port = if (uri.port == -1 || uri.port == 443) "" else ":${uri.port}"
        val path = uri.rawPath.takeUnless { it.isNullOrEmpty() } ?: "/"
        val query = uri.rawQuery?.let { "?$it" } ?: ""
        return "https://$host$port$path$query"
    }
}
