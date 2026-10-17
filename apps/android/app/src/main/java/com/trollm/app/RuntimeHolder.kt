package com.trollm.app

import android.content.Context
import uniffi.trollm_uniffi.TrollmRuntime
import uniffi.trollm_uniffi.TokenProvider
import uniffi.trollm_uniffi.ConnectionAuth

/**
 * Process-wide holder for the Rust runtime. The UniFFI object wires the
 * §8 storage tree under `filesDir/trollm`; the [TokenProvider] callback
 * resolves MCP bearer tokens from [SecretsStore] — auth is resolved by the
 * app, never by the plan (§3).
 */
object RuntimeHolder {
    @Volatile
    private var instance: TrollmRuntime? = null

    fun get(context: Context): TrollmRuntime =
        instance ?: synchronized(this) {
            instance ?: run {
                val appContext = context.applicationContext
                val secrets = SecretsStore(appContext)
                val provider = object : TokenProvider {
                    override fun bearerToken(serverName: String, serverUrl: String): String? = try {
                        secrets.token(serverName, serverUrl)
                    } catch (_: Exception) { null }

                    override fun connectionAuth(serverName: String, serverUrl: String): ConnectionAuth = try {
                        when (secrets.selectedAuthMode(serverName)) {
                            SelectedAuthMode.ANONYMOUS -> ConnectionAuth.Anonymous
                            SelectedAuthMode.BEARER -> if (secrets.token(serverName, serverUrl) != null)
                                ConnectionAuth.Bearer else ConnectionAuth.Unavailable
                            SelectedAuthMode.OAUTH -> OAuthConnections.get(appContext).pin(serverName, serverUrl)
                                ?.let { ConnectionAuth.Oauth(it.grantId) } ?: ConnectionAuth.Unavailable
                            SelectedAuthMode.UNAVAILABLE -> ConnectionAuth.Unavailable
                        }
                    } catch (_: Exception) { ConnectionAuth.Unavailable }

                    override fun oauthToken(serverName: String, serverUrl: String, grantId: String): String? = try {
                        OAuthConnections.get(appContext).accessTokenBlocking(serverName, serverUrl, grantId)
                    } catch (_: Exception) { null }
                }
                TrollmRuntime(appContext.filesDir.absolutePath, provider)
                    .also { instance = it }
            }
        }
}
