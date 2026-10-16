package com.trollm.app

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import net.openid.appauth.AppAuthConfiguration
import net.openid.appauth.AuthState
import net.openid.appauth.AuthorizationException
import net.openid.appauth.AuthorizationRequest
import net.openid.appauth.AuthorizationResponse
import net.openid.appauth.AuthorizationService
import net.openid.appauth.AuthorizationServiceConfiguration
import net.openid.appauth.CodeVerifierUtil
import net.openid.appauth.RegistrationRequest
import net.openid.appauth.RegistrationResponse
import net.openid.appauth.ResponseTypeValues
import net.openid.appauth.TokenResponse
import org.json.JSONObject
import java.net.URI
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume

/** App-owned OAuth only. Legacy manually provisioned bearer tokens remain separate. */
class OAuthConnections private constructor(private val context: Context) {
    private val grants = SecretsStore(context).oauthGrants
    private val connections = OAuthConnectionBuilder(context.filesDir)
    private val main = Handler(Looper.getMainLooper())
    private val persistence = Executors.newSingleThreadExecutor()
    private val pinnedServers = ConcurrentHashMap<String, String>()
    private val refreshes = ConcurrentHashMap<String, CompletableFuture<String?>>()
    // SDK interaction is marshalled onto main; token persistence stays off main.
    private val service by lazy {
        AuthorizationService(context, AppAuthConfiguration.Builder()
            .setConnectionBuilder(connections).build())
    }

    fun pin(serverName: String, serverUrl: String): PinnedOAuthGrant? {
        val pin = grants.pin(serverName, serverUrl) ?: return null
        return if (bindGrant(serverName, serverUrl, pin.grantId)) pin else null
    }

    /** Register only the exact saved grant when a worker/cold resume supplies its ID. */
    fun bindGrant(serverName: String, serverUrl: String, grantId: String): Boolean {
        val pin = grants.bind(serverName, serverUrl, grantId) ?: return false
        val previous = pinnedServers.putIfAbsent(pin.grantId, serverName)
        return previous == null || previous == serverName
    }

    fun accessTokenBlocking(serverName: String, serverUrl: String, grantId: String): String? {
        return if (bindGrant(serverName, serverUrl, grantId)) accessTokenBlocking(grantId, serverUrl)
            else null
    }

    /** Worker-thread callback only. No token/account substitution or request replay. */
    fun accessTokenBlocking(grantId: String, serverUrl: String): String? {
        check(Looper.myLooper() != Looper.getMainLooper()) { "Token access must run off the UI thread" }
        return try {
            val endpoint = ConnectionInput.canonicalEndpoint(serverUrl)
            val server = pinnedServers[grantId] ?: return null
            val future = refreshes.computeIfAbsent(grantId) {
                CompletableFuture<String?>().also { pending ->
                    persistence.execute {
                        try {
                            // Read after earlier refresh persistence, not before entering
                            // single-flight; a rotated refresh token must not roll back.
                            val grant = grants.matching(server, grantId, endpoint)
                                ?.takeIf { it.pendingRequest == null }
                            if (grant == null) {
                                pending.complete(null)
                                refreshes.remove(grantId, pending)
                                return@execute
                            }
                            val state = AuthState.jsonDeserialize(grant.state)
                            main.post {
                                try {
                                    state.performActionWithFreshTokens(service) { token, _, error ->
                                        persistence.execute {
                                            try {
                                                val saved = grants.update(server, grantId, endpoint, state.jsonSerializeString())
                                                pending.complete(if (saved && error == null && token != null &&
                                                    ConnectionInput.validToken(token)) token else null)
                                            } catch (_: Exception) {
                                                pending.complete(null)
                                            } finally {
                                                refreshes.remove(grantId, pending)
                                            }
                                        }
                                    }
                                } catch (_: Exception) {
                                    pending.complete(null)
                                    refreshes.remove(grantId, pending)
                                }
                            }
                        } catch (_: Exception) {
                            pending.complete(null)
                            refreshes.remove(grantId, pending)
                        }
                    }
                }
            }
            val token = future.get(20, TimeUnit.SECONDS)
            // Logout/replacement while waiting invalidates even a completed refresh.
            if (grants.matching(server, grantId, endpoint) != null) token else null
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            null
        } catch (_: Exception) {
            null
        }
    }

    fun status(server: String, endpoint: String): String {
        when (grants.mode(server)) {
            SelectedAuthMode.BEARER -> return "Manual bearer authentication selected."
            SelectedAuthMode.ANONYMOUS -> return "No authentication selected."
            SelectedAuthMode.UNAVAILABLE -> return "Sign in again or save a manual token to reconnect."
            SelectedAuthMode.OAUTH -> Unit
        }
        val grant = grants.current(server) ?: return "Not signed in with OAuth."
        if (grant.endpoint != ConnectionInput.canonicalEndpoint(endpoint)) {
            return "OAuth is configured for a different MCP endpoint."
        }
        if (grant.pendingRequest != null) return "Sign-in pending."
        val state = AuthState.jsonDeserialize(grant.state)
        val expiredWithoutRefresh = state.refreshToken == null &&
            state.accessTokenExpirationTime?.let { it <= System.currentTimeMillis() + 60_000 } == true
        return if (state.isAuthorized && !expiredWithoutRefresh) "OAuth account connected."
            else "Sign in again to reconnect this account."
    }

    fun logout(server: String) {
        grants.logout(server)
    }

    /** Browser PKCE: RFC8414 metadata is mapped explicitly, never parsed as OIDC discovery. */
    suspend fun signIn(server: String, endpointInput: String) {
        require(ConnectionInput.validServer(server))
        val endpoint = ConnectionInput.canonicalEndpoint(endpointInput)
        // Select OAuth before discovery/registration. A failed attempt cannot
        // quietly reactivate a previously saved manual credential.
        val grant = withContext(Dispatchers.IO) { grants.begin(server, endpoint, "", "preparing") }
        try {
            val metadata = withContext(Dispatchers.IO) { metadata(endpoint) }
            val configuration = AuthorizationServiceConfiguration(
                metadata.authorization, metadata.token, metadata.registration)
            val registration = withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<RegistrationResponse> { continuation ->
                    service.performRegistrationRequest(
                        RegistrationRequest.Builder(configuration, listOf(REDIRECT))
                            .setTokenEndpointAuthenticationMethod("none")
                            .setGrantTypeValues("authorization_code", "refresh_token")
                            .setResponseTypeValues("code")
                            .setAdditionalParameters(mapOf("client_name" to "Trollm Android"))
                            .build(),
                    ) { response, _ ->
                        if (continuation.isActive) {
                            if (response != null) continuation.resume(response)
                            else continuation.resumeWith(Result.failure(IllegalStateException("Sign-in setup failed")))
                        }
                    }
                }
            }
            val request = AuthorizationRequest.Builder(configuration, registration.clientId,
                ResponseTypeValues.CODE, REDIRECT)
                .setScope(metadata.scope)
                .setCodeVerifier(CodeVerifierUtil.generateRandomCodeVerifier())
                .build()
            withContext(Dispatchers.IO) {
                check(grants.prepare(server, grant.id, endpoint, AuthState(registration).jsonSerializeString(),
                    request.jsonSerializeString())) { "Sign-in selection changed" }
            }
            withContext(Dispatchers.Main) {
                service.performAuthorizationRequest(request, resultIntent(grant, false), resultIntent(grant, true))
            }
        } catch (error: Exception) {
            withContext(NonCancellable + Dispatchers.IO) { grants.invalidate(server, grant.id) }
            throw error
        }
    }

    /** Invoked only by our non-exported activity, through SDK completion intents. */
    suspend fun complete(intent: Intent): Boolean {
        val server = intent.getStringExtra(EXTRA_SERVER) ?: return false
        val id = intent.getStringExtra(EXTRA_GRANT) ?: return false
        val grant = withContext(Dispatchers.IO) { grants.current(server)?.takeIf { it.id == id } }
            ?: return false
        try {
            if (intent.getBooleanExtra(EXTRA_CANCEL, false)) return false
            val response = AuthorizationResponse.fromIntent(intent) ?: return false
            if (AuthorizationException.fromIntent(intent) != null || grant.pendingRequest == null) return false
            val expected = AuthorizationRequest.jsonDeserialize(grant.pendingRequest)
            require(response.request.state == expected.state &&
                response.request.codeVerifier == expected.codeVerifier &&
                response.request.clientId == expected.clientId && response.request.redirectUri == REDIRECT &&
                response.request.configuration.tokenEndpoint == expected.configuration.tokenEndpoint &&
                response.request.configuration.authorizationEndpoint == expected.configuration.authorizationEndpoint)
            val state = AuthState.jsonDeserialize(grant.state)
            state.update(response, null)
            val token = withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<Pair<TokenResponse?, AuthorizationException?>> { continuation ->
                    service.performTokenRequest(response.createTokenExchangeRequest()) { result, error ->
                        if (continuation.isActive) continuation.resume(result to error)
                    }
                }
            }
            state.update(token.first, token.second)
            require(token.second == null && state.accessToken?.let(ConnectionInput::validToken) == true)
            return withContext(Dispatchers.IO) {
                grants.update(server, id, grant.endpoint, state.jsonSerializeString())
            }
        } finally {
            // Do not invalidate a successful completion or a newer replacement login.
            withContext(NonCancellable + Dispatchers.IO) {
                val current = grants.matching(server, id, grant.endpoint)
                if (current?.pendingRequest != null) grants.invalidate(server, id)
            }
        }
    }

    private fun resultIntent(grant: OAuthGrant, cancelled: Boolean): PendingIntent {
        val intent = Intent(context, OAuthCallbackActivity::class.java)
            .setAction(if (cancelled) "com.trollm.app.OAUTH_CANCEL" else "com.trollm.app.OAUTH_COMPLETE")
            .setData(Uri.parse("com.trollm.app:oauth-result/${grant.id}"))
            .putExtra(EXTRA_SERVER, grant.server).putExtra(EXTRA_GRANT, grant.id)
            .putExtra(EXTRA_CANCEL, cancelled)
        // AppAuth must fill in its response; the destination is explicit and non-exported.
        return PendingIntent.getActivity(context, 0, intent,
            PendingIntent.FLAG_CANCEL_CURRENT or PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_ONE_SHOT)
    }

    private data class Metadata(val authorization: Uri, val token: Uri, val registration: Uri, val scope: String)

    private fun metadata(endpoint: String): Metadata {
        val origin = URI(endpoint).let { "https://${it.rawAuthority}" }
        val connection = connections.openConnection(Uri.parse("$origin/.well-known/oauth-authorization-server"))
        val value = try {
            connection.setRequestProperty("Accept", "application/json")
            require(connection.responseCode == 200) { "OAuth metadata unavailable" }
            val bytes = connection.inputStream.use { input ->
                val buffer = ByteArray(64 * 1024 + 1)
                var size = 0
                while (size < buffer.size) {
                    val read = input.read(buffer, size, buffer.size - size)
                    if (read == -1) break
                    size += read
                }
                require(size <= 64 * 1024) { "OAuth metadata too large" }
                buffer.copyOf(size)
            }
            JSONObject(bytes.toString(Charsets.UTF_8))
        } finally { connection.disconnect() }
        require(ConnectionInput.canonicalEndpoint(value.getString("issuer")) == "$origin/") {
            "Unexpected OAuth issuer"
        }
        fun endpointUri(key: String): Uri {
            val canonical = ConnectionInput.canonicalEndpoint(value.getString(key))
            require(URI(canonical).rawAuthority == URI(origin).rawAuthority) { "Unexpected OAuth endpoint" }
            return Uri.parse(canonical)
        }
        val scopes = value.optJSONArray("scopes_supported")
        val offline = (0 until (scopes?.length() ?: 0)).any { scopes?.optString(it) == "offline_access" }
        // Access-token OAuth only; the facade does not forward an OIDC nonce.
        // Keep the SDK's normal checks, and do not request an unused ID token.
        val scope = "mcp:e2llm" + if (offline) " offline_access" else ""
        return Metadata(endpointUri("authorization_endpoint"), endpointUri("token_endpoint"),
            endpointUri("registration_endpoint"), scope)
    }

    companion object {
        val REDIRECT: Uri = Uri.parse("com.trollm.app://oauth/callback")
        private const val EXTRA_SERVER = "oauth_server"
        private const val EXTRA_GRANT = "oauth_grant"
        private const val EXTRA_CANCEL = "oauth_cancel"
        @Volatile private var instance: OAuthConnections? = null
        fun get(context: Context): OAuthConnections = instance ?: synchronized(this) {
            instance ?: OAuthConnections(context.applicationContext).also { instance = it }
        }
    }
}
