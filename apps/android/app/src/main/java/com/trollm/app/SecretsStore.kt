package com.trollm.app

import android.content.Context
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import org.json.JSONObject

/**
 * MCP bearer tokens, stored in Android Keystore-backed
 * EncryptedSharedPreferences (§10, SECURITY-ENTERPRISE.md "TLS-only;
 * tokens in Android Keystore-backed storage").
 *
 * Keys are per MCP server name (`token.<server>`), matching the plan's
 * `mcp.servers[].name`. Values never touch logs or the event log.
 */
class SecretsStore(context: Context) {
    private val masterKey = MasterKey.Builder(context)
        .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
        .build()

    private val prefs = EncryptedSharedPreferences.create(
        context,
        "trollm_mcp_tokens",
        masterKey,
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    fun setToken(server: String, token: String?) {
        oauthGrants.selectManual(server, token)
    }

    fun token(server: String): String? = prefs.getString(key(server), null)

    fun hasToken(server: String): Boolean = prefs.contains(key(server))

    val oauthGrants = OAuthGrants(::readConnection, ::writeConnection, oauthLock)

    fun selectedAuthMode(server: String): SelectedAuthMode = oauthGrants.mode(server)

    private fun readConnection(server: String): StoredConnection {
        val mode = prefs.getString("auth-mode.$server", null)?.let {
            SelectedAuthMode.entries.find { mode -> mode.name == it } ?: SelectedAuthMode.UNAVAILABLE
        }
        return StoredConnection(mode, token(server), readOAuth(server))
    }

    private fun readOAuth(server: String): OAuthGrant? {
        val encoded = prefs.getString("oauth.$server", null) ?: return null
        val value = JSONObject(encoded)
        return OAuthGrant(value.getString("id"), server, value.getString("endpoint"),
            value.getString("state"), if (value.has("pendingRequest")) value.getString("pendingRequest") else null)
    }

    private fun writeConnection(server: String, stored: StoredConnection) {
        val editor = prefs.edit()
        editor.putString("auth-mode.$server", checkNotNull(stored.mode).name)
        if (stored.manualToken == null) editor.remove(key(server))
        else editor.putString(key(server), stored.manualToken)
        val grant = stored.grant
        if (grant == null) {
            editor.remove("oauth.$server")
        } else {
            val value = JSONObject().put("id", grant.id).put("endpoint", grant.endpoint)
                .put("state", grant.state)
            grant.pendingRequest?.let { value.put("pendingRequest", it) }
            editor.putString("oauth.$server", value.toString())
        }
        check(editor.commit()) { "Could not update connection storage" }
    }

    private fun key(server: String) = "token.$server"

    companion object {
        private val oauthLock = Any()
    }
}
