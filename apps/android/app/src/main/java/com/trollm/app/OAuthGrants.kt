package com.trollm.app

import java.util.UUID

/** Contains secrets; deliberately has no generated data-class toString/copy output. */
class OAuthGrant(
    val id: String,
    val server: String,
    val endpoint: String,
    val state: String,
    val pendingRequest: String? = null,
) {
    override fun toString() = "OAuthGrant(redacted)"
}

data class PinnedOAuthGrant(val grantId: String, val serverUrl: String)

enum class SelectedAuthMode { ANONYMOUS, BEARER, OAUTH, UNAVAILABLE }

/** One encrypted transaction includes selection, legacy manual token and grant. */
class StoredConnection(val mode: SelectedAuthMode?, val manualToken: String?, val grant: OAuthGrant?) {
    override fun toString() = "StoredConnection(redacted)"
}

/** One active grant per connection. All store instances share the supplied lock. */
class OAuthGrants(
    private val read: (String) -> StoredConnection,
    private val write: (String, StoredConnection) -> Unit,
    private val lock: Any,
) {
    fun begin(server: String, endpoint: String, state: String, request: String): OAuthGrant =
        synchronized(lock) {
            require(ConnectionInput.validServer(server))
            OAuthGrant(UUID.randomUUID().toString(), server, ConnectionInput.canonicalEndpoint(endpoint),
                state, request).also {
                    write(server, StoredConnection(SelectedAuthMode.OAUTH, read(server).manualToken, it))
                }
        }

    private fun selected(server: String): StoredConnection {
        val stored = read(server)
        if (stored.mode != null) return stored
        // Migration happens once. A retained OAuth tombstone must never become Bearer.
        val mode = when {
            stored.grant != null -> SelectedAuthMode.OAUTH
            stored.manualToken != null -> SelectedAuthMode.BEARER
            else -> SelectedAuthMode.ANONYMOUS
        }
        return StoredConnection(mode, stored.manualToken, stored.grant).also { write(server, it) }
    }

    fun mode(server: String): SelectedAuthMode = synchronized(lock) { selected(server).mode!! }

    fun current(server: String): OAuthGrant? = synchronized(lock) {
        selected(server).takeIf { it.mode == SelectedAuthMode.OAUTH }?.grant
    }

    fun selectManual(server: String, token: String?) = synchronized(lock) {
        write(server, StoredConnection(if (token == null) SelectedAuthMode.ANONYMOUS
            else SelectedAuthMode.BEARER, token, null))
    }

    fun pin(server: String, endpoint: String): PinnedOAuthGrant? = synchronized(lock) {
        val canonical = ConnectionInput.canonicalEndpoint(endpoint)
        current(server)?.takeIf { it.endpoint == canonical && it.pendingRequest == null }
            ?.let { PinnedOAuthGrant(it.id, canonical) }
    }

    /** Cold resume supplies the saved ID; this never substitutes the latest login. */
    fun bind(server: String, endpoint: String, id: String): PinnedOAuthGrant? = synchronized(lock) {
        val canonical = ConnectionInput.canonicalEndpoint(endpoint)
        matching(server, id, canonical)?.takeIf { it.pendingRequest == null }
            ?.let { PinnedOAuthGrant(it.id, canonical) }
    }

    fun prepare(server: String, id: String, endpoint: String, state: String, request: String): Boolean =
        synchronized(lock) {
            if (matching(server, id, endpoint)?.pendingRequest == null) return@synchronized false
            write(server, StoredConnection(SelectedAuthMode.OAUTH, read(server).manualToken,
                OAuthGrant(id, server, endpoint, state, request)))
            true
        }

    fun matching(server: String, id: String, endpoint: String): OAuthGrant? = synchronized(lock) {
        current(server)?.takeIf { it.id == id && it.endpoint == endpoint }
    }

    /** A late exchange/refresh can update only its own still-active grant. */
    fun update(server: String, id: String, endpoint: String, state: String): Boolean =
        synchronized(lock) {
            if (matching(server, id, endpoint) == null) return@synchronized false
            write(server, StoredConnection(SelectedAuthMode.OAUTH, read(server).manualToken,
                OAuthGrant(id, server, endpoint, state)))
            true
        }

    fun logout(server: String) = synchronized(lock) {
        write(server, StoredConnection(SelectedAuthMode.UNAVAILABLE, read(server).manualToken, null))
    }

    fun invalidate(server: String, id: String) = synchronized(lock) {
        if (current(server)?.id == id) logout(server)
    }
}
