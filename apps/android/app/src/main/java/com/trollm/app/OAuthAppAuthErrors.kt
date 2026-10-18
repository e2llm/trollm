package com.trollm.app

import net.openid.appauth.AuthorizationException
import java.net.SocketTimeoutException

/** Map only protocol/SDK identifiers; provider descriptions and URIs stay private. */
fun appAuthProblem(error: AuthorizationException?): OAuthProblem {
    if (error == null) return OAuthProblem.FAILED
    fun matches(template: AuthorizationException): Boolean =
        error.type == template.type && error.code == template.code
    return when {
        matches(AuthorizationException.GeneralErrors.USER_CANCELED_AUTH_FLOW) ||
            matches(AuthorizationException.GeneralErrors.PROGRAM_CANCELED_AUTH_FLOW) ||
            matches(AuthorizationException.AuthorizationRequestErrors.ACCESS_DENIED) ||
            (error.type == AuthorizationException.TYPE_OAUTH_AUTHORIZATION_ERROR && error.error == "access_denied") ->
            OAuthProblem.CANCELLED
        matches(AuthorizationException.GeneralErrors.NETWORK_ERROR) ->
            if (error.cause is SocketTimeoutException) OAuthProblem.TIMEOUT else OAuthProblem.NETWORK
        matches(AuthorizationException.TokenRequestErrors.INVALID_GRANT) ||
            (error.type == AuthorizationException.TYPE_OAUTH_TOKEN_ERROR && error.error == "invalid_grant") ->
            OAuthProblem.RECONNECT
        matches(AuthorizationException.GeneralErrors.INVALID_DISCOVERY_DOCUMENT) ||
            matches(AuthorizationException.GeneralErrors.JSON_DESERIALIZATION_ERROR) ||
            matches(AuthorizationException.GeneralErrors.INVALID_REGISTRATION_RESPONSE) ||
            matches(AuthorizationException.GeneralErrors.TOKEN_RESPONSE_CONSTRUCTION_ERROR) ||
            matches(AuthorizationException.AuthorizationRequestErrors.STATE_MISMATCH) ||
            matches(AuthorizationException.AuthorizationRequestErrors.INVALID_REQUEST) ||
            matches(AuthorizationException.AuthorizationRequestErrors.UNAUTHORIZED_CLIENT) ||
            matches(AuthorizationException.AuthorizationRequestErrors.INVALID_SCOPE) ||
            matches(AuthorizationException.AuthorizationRequestErrors.UNSUPPORTED_RESPONSE_TYPE) ||
            matches(AuthorizationException.RegistrationRequestErrors.INVALID_REQUEST) ||
            matches(AuthorizationException.RegistrationRequestErrors.INVALID_REDIRECT_URI) ||
            matches(AuthorizationException.RegistrationRequestErrors.INVALID_CLIENT_METADATA) ||
            matches(AuthorizationException.TokenRequestErrors.INVALID_CLIENT) ||
            matches(AuthorizationException.TokenRequestErrors.UNAUTHORIZED_CLIENT) ||
            matches(AuthorizationException.TokenRequestErrors.INVALID_SCOPE) ||
            matches(AuthorizationException.TokenRequestErrors.UNSUPPORTED_GRANT_TYPE) ->
            OAuthProblem.CONFIGURATION
        else -> OAuthProblem.FAILED
    }
}
