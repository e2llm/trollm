package com.trollm.app

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.Dispatchers
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
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CancellationException
import java.util.concurrent.Executors
import kotlin.coroutines.resume

/** App-owned OAuth only. Legacy manually provisioned bearer tokens remain separate. */
class OAuthConnections private constructor(private val context: Context, private val source: Boolean = false) {
    private val grants = storage { SecretsStore(context, source).oauthGrants }
    private val connections = OAuthConnectionBuilder(context.filesDir, source)
    private val main = Handler(Looper.getMainLooper())
    private val persistence = Executors.newSingleThreadExecutor()
    private val pinnedServers = ConcurrentHashMap<String, String>()
    private val refresh = OAuthRefresh(grants, { work -> persistence.execute { work() } }, ::freshToken)
    // SDK interaction is marshalled onto main; token persistence stays off main.
    private val service by lazy {
        AuthorizationService(context, AppAuthConfiguration.Builder()
            .setConnectionBuilder(connections).build())
    }

    fun pin(serverName: String, serverUrl: String): PinnedOAuthGrant? {
        val pin = storage { grants.pin(serverName, serverUrl) } ?: return null
        return if (bindGrant(serverName, serverUrl, pin.grantId)) pin else null
    }

    /** Register only the exact saved grant when a worker/cold resume supplies its ID. */
    fun bindGrant(serverName: String, serverUrl: String, grantId: String): Boolean {
        val pin = storage { grants.bind(serverName, serverUrl, grantId) } ?: return false
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
        val endpoint = try { ConnectionInput.canonicalEndpoint(serverUrl) } catch (_: Exception) { return null }
        val server = pinnedServers[grantId] ?: return null
        return refresh.accessTokenBlocking(server, endpoint, grantId)
    }

    /** AppAuth owns token refresh; the pure coordinator owns save/identity checks. */
    private fun freshToken(grant: OAuthGrant, complete: (OAuthRefreshResult) -> Unit) {
        val state = configuration { AuthState.jsonDeserialize(grant.state) }
        if (!main.post {
            try {
                state.performActionWithFreshTokens(service) { token, _, error ->
                    try {
                        // Preserve SDK state even on a failed refresh (e.g. invalid_grant).
                        complete(OAuthRefreshResult(token, state.jsonSerializeString(),
                            error?.let(::appAuthProblem)))
                    } catch (failure: Exception) {
                        complete(OAuthRefreshResult(null, null, if (failure is CancellationException) OAuthProblem.CANCELLED else classifyOAuthProblem(failure)))
                    }
                }
            } catch (failure: Exception) {
                complete(OAuthRefreshResult(null, null, if (failure is CancellationException) OAuthProblem.CANCELLED else classifyOAuthProblem(failure)))
            }
        }) {
            complete(OAuthRefreshResult(null, null, OAuthProblem.FAILED))
        }
    }

    fun status(server: String, endpoint: String): String {
        when (storage { grants.mode(server) }) {
            SelectedAuthMode.BEARER -> return "Manual bearer authentication selected."
            SelectedAuthMode.ANONYMOUS -> return "No authentication selected."
            SelectedAuthMode.UNAVAILABLE -> return "Sign in again or save a manual token to reconnect."
            SelectedAuthMode.OAUTH -> Unit
        }
        val grant = storage { grants.current(server) } ?: return "Not signed in with OAuth."
        if (grant.endpoint != configuration { ConnectionInput.canonicalEndpoint(endpoint) }) {
            return "OAuth is configured for a different MCP endpoint."
        }
        if (grant.pendingRequest != null) return "Sign-in pending."
        refresh.problem(server, grant.endpoint, grant.id)?.let { return it.message }
        val state = configuration { AuthState.jsonDeserialize(grant.state) }
        val expiredWithoutRefresh = state.refreshToken == null &&
            state.accessTokenExpirationTime?.let { it <= System.currentTimeMillis() + 60_000 } == true
        return if (state.isAuthorized && !expiredWithoutRefresh) "OAuth account connected."
            else "Sign in again to reconnect this account."
    }

    fun logout(server: String) {
        storage { grants.logout(server) }
        refresh.clearServer(server)
    }

    private inline fun <T> storage(action: () -> T): T = try {
        action()
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (safe: OAuthProblemException) {
        throw safe
    } catch (_: Exception) {
        throw OAuthProblemException(OAuthProblem.STORAGE)
    }

    private inline fun <T> configuration(action: () -> T): T = try {
        action()
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (safe: OAuthProblemException) {
        throw safe
    } catch (_: Exception) {
        throw OAuthProblemException(OAuthProblem.CONFIGURATION)
    }

    /** Browser PKCE: RFC8414 metadata is mapped explicitly, never parsed as OIDC discovery. */
    suspend fun signIn(server: String, endpointInput: String) {
        require(!source)
        beginSignIn(server, endpointInput, null)
    }

    suspend fun signInMoray(profile: MorayProfile) {
        require(source)
        beginSignIn("moray-${profile.key}", profile.api, profile)
    }

    private suspend fun beginSignIn(server: String, endpointInput: String, profile: MorayProfile?) {
        if (!ConnectionInput.validServer(server)) throw OAuthProblemException(OAuthProblem.CONFIGURATION)
        val endpoint = configuration { ConnectionInput.canonicalEndpoint(endpointInput) }
        withPendingOAuthGrant(
            begin = { storage { grants.begin(server, endpoint, "", "preparing") } },
            invalidate = { grant -> storage { grants.invalidatePending(server, grant.id) } },
        ) { grant ->
            refresh.clearServer(server)
            val metadata = withContext(Dispatchers.IO) {
                try { if (profile == null) metadata(endpoint) else sourceMetadata(profile) } catch (cancelled: CancellationException) { throw cancelled }
                catch (failure: Exception) { throw OAuthProblemException(classifyOAuthProblem(failure)) }
            }
            val sdkConfiguration = configuration {
                AuthorizationServiceConfiguration(metadata.authorization, metadata.token, metadata.registration)
            }
            val registration = if (profile != null) null else withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<RegistrationResponse> { continuation ->
                    try {
                        service.performRegistrationRequest(
                            RegistrationRequest.Builder(sdkConfiguration, listOf(REDIRECT))
                                .setTokenEndpointAuthenticationMethod("none")
                                .setGrantTypeValues("authorization_code", "refresh_token")
                                .setResponseTypeValues("code")
                                .setAdditionalParameters(mapOf("client_name" to "Trollm Android"))
                                .build(),
                        ) { response, error ->
                            if (continuation.isActive) {
                                if (response != null && error == null) continuation.resume(response)
                                else continuation.resumeWith(Result.failure(OAuthProblemException(appAuthProblem(error))))
                            }
                        }
                    } catch (failure: Exception) {
                        if (continuation.isActive) continuation.resumeWith(
                            Result.failure(OAuthProblemException(classifyOAuthProblem(failure))))
                    }
                }
            }
            val request = configuration {
                AuthorizationRequest.Builder(sdkConfiguration, profile?.clientId ?: checkNotNull(registration).clientId, ResponseTypeValues.CODE, REDIRECT)
                    .setScope(metadata.scope)
                    .setCodeVerifier(CodeVerifierUtil.generateRandomCodeVerifier())
                    .build()
            }
            withContext(Dispatchers.IO) {
                val saved = storage { grants.prepare(server, grant.id, endpoint,
                    configuration { (if (registration != null) AuthState(registration) else AuthState(sdkConfiguration)).jsonSerializeString() }, request.jsonSerializeString()) }
                if (!saved) throw OAuthProblemException(OAuthProblem.RECONNECT)
            }
            withContext(Dispatchers.Main) {
                try { service.performAuthorizationRequest(request, resultIntent(grant, false), resultIntent(grant, true)) }
                catch (failure: Exception) { throw OAuthProblemException(classifyOAuthProblem(failure)) }
            }
        }
    }

    /** Invoked only by our non-exported activity, through SDK completion intents. */
    suspend fun complete(intent: Intent): Boolean {
        val server = intent.getStringExtra(EXTRA_SERVER) ?: throw OAuthProblemException(OAuthProblem.CONFIGURATION)
        val id = intent.getStringExtra(EXTRA_GRANT) ?: throw OAuthProblemException(OAuthProblem.CONFIGURATION)
        return withOAuthCleanup(cleanup = {
            storage {
                // One store transaction preserves completed and replacement grants.
                grants.invalidatePending(server, id)
            }
        }) {
            val grant = withContext(Dispatchers.IO) {
                storage { grants.current(server)?.takeIf { it.id == id } }
            } ?: throw OAuthProblemException(OAuthProblem.RECONNECT)
            if (intent.getBooleanExtra(EXTRA_CANCEL, false)) throw OAuthProblemException(OAuthProblem.CANCELLED)
            configuration { AuthorizationException.fromIntent(intent) }?.let {
                throw OAuthProblemException(appAuthProblem(it))
            }
            val response = configuration { AuthorizationResponse.fromIntent(intent) }
                ?: throw OAuthProblemException(OAuthProblem.FAILED)
            val pending = grant.pendingRequest ?: throw OAuthProblemException(OAuthProblem.RECONNECT)
            val expected = configuration { AuthorizationRequest.jsonDeserialize(pending) }
            configuration {
                require(response.request.state == expected.state &&
                    response.request.codeVerifier == expected.codeVerifier &&
                    response.request.clientId == expected.clientId && response.request.redirectUri == REDIRECT &&
                    response.request.configuration.tokenEndpoint == expected.configuration.tokenEndpoint &&
                    response.request.configuration.authorizationEndpoint == expected.configuration.authorizationEndpoint)
            }
            val state = configuration { AuthState.jsonDeserialize(grant.state) }
            state.update(response, null)
            val token = withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<Pair<TokenResponse?, AuthorizationException?>> { continuation ->
                    try {
                        service.performTokenRequest(response.createTokenExchangeRequest()) { result, error ->
                            if (continuation.isActive) continuation.resume(result to error)
                        }
                    } catch (failure: Exception) {
                        if (continuation.isActive) continuation.resumeWith(
                            Result.failure(OAuthProblemException(classifyOAuthProblem(failure))))
                    }
                }
            }
            configuration { state.update(token.first, token.second) }
            token.second?.let { throw OAuthProblemException(appAuthProblem(it)) }
            if (state.accessToken?.let(ConnectionInput::validToken) != true) throw OAuthProblemException(OAuthProblem.FAILED)
            val saved = withContext(Dispatchers.IO) {
                storage { grants.update(server, id, grant.endpoint, configuration { state.jsonSerializeString() }) }
            }
            if (!saved) throw OAuthProblemException(OAuthProblem.RECONNECT)
            true
        }
    }

    private fun resultIntent(grant: OAuthGrant, cancelled: Boolean): PendingIntent {
        val intent = Intent(context, OAuthCallbackActivity::class.java)
            .setAction(if (cancelled) "com.trollm.app.OAUTH_CANCEL" else "com.trollm.app.OAUTH_COMPLETE")
            .setData(Uri.parse("com.trollm.app:oauth-result/${grant.id}"))
            .putExtra(EXTRA_SERVER, grant.server).putExtra(EXTRA_GRANT, grant.id)
            .putExtra(EXTRA_CANCEL, cancelled).putExtra(EXTRA_SOURCE, source)
        // AppAuth must fill in its response; the destination is explicit and non-exported.
        return PendingIntent.getActivity(context, 0, intent,
            PendingIntent.FLAG_CANCEL_CURRENT or PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_ONE_SHOT)
    }

    private data class Metadata(val authorization: Uri, val token: Uri, val registration: Uri?, val scope: String)

    private fun metadata(endpoint: String): Metadata = try {
        readMetadata(endpoint)
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (failure: java.io.IOException) {
        throw OAuthProblemException(classifyOAuthProblem(failure))
    } catch (_: Exception) {
        throw OAuthProblemException(OAuthProblem.CONFIGURATION)
    }

    private fun readMetadata(endpoint: String): Metadata {
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

    private fun sourceMetadata(profile: MorayProfile): Metadata {
        val connection = connections.openConnection(Uri.parse(profile.issuer + "/.well-known/openid-configuration"))
        val value = try {
            require(connection.responseCode == 200)
            JSONObject(connection.inputStream.use { BundleInput.read(it, 64 * 1024) }.toString(Charsets.UTF_8))
        } finally { connection.disconnect() }
        require(value.getString("issuer") == profile.issuer)
        val methods = value.getJSONArray("code_challenge_methods_supported")
        require((0 until methods.length()).any { methods.getString(it) == "S256" })
        fun endpoint(name: String): Uri {
            val text = ConnectionInput.canonicalEndpoint(value.getString(name))
            require(URI(text).rawAuthority == URI(profile.issuer).rawAuthority)
            return Uri.parse(text)
        }
        return Metadata(endpoint("authorization_endpoint"), endpoint("token_endpoint"), null,
            "moray:bundle:read moray:edge-report:write offline_access")
    }

    companion object {
        val REDIRECT: Uri = Uri.parse("com.trollm.app://oauth/callback")
        private const val EXTRA_SERVER = "oauth_server"
        private const val EXTRA_GRANT = "oauth_grant"
        private const val EXTRA_CANCEL = "oauth_cancel"
        const val EXTRA_SOURCE = "oauth_source"
        @Volatile private var instance: OAuthConnections? = null
        @Volatile private var sourceInstance: OAuthConnections? = null
        fun moray(context: Context): OAuthConnections = sourceInstance ?: synchronized(this) {
            sourceInstance ?: OAuthConnections(context.applicationContext, true).also { sourceInstance = it }
        }
        fun get(context: Context): OAuthConnections = instance ?: synchronized(this) {
            instance ?: OAuthConnections(context.applicationContext).also { instance = it }
        }
    }
}
