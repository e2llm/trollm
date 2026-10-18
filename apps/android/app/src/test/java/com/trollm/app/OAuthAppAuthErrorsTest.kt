package com.trollm.app

import net.openid.appauth.AuthorizationException
import org.junit.Assert.*
import org.junit.Test
import java.net.SocketTimeoutException

class OAuthAppAuthErrorsTest {
    @Test fun sdkCodesAndProtocolIdentifiersMapWithoutProviderText() {
        assertEquals(OAuthProblem.CANCELLED, appAuthProblem(AuthorizationException.GeneralErrors.USER_CANCELED_AUTH_FLOW))
        assertEquals(OAuthProblem.CANCELLED, appAuthProblem(AuthorizationException.AuthorizationRequestErrors.ACCESS_DENIED))
        assertEquals(OAuthProblem.RECONNECT, appAuthProblem(AuthorizationException.TokenRequestErrors.INVALID_GRANT))
        assertEquals(OAuthProblem.NETWORK, appAuthProblem(AuthorizationException.GeneralErrors.NETWORK_ERROR))
        assertEquals(OAuthProblem.TIMEOUT, appAuthProblem(AuthorizationException.fromTemplate(
            AuthorizationException.GeneralErrors.NETWORK_ERROR, SocketTimeoutException("private URL"))))
        assertEquals(OAuthProblem.CONFIGURATION, appAuthProblem(AuthorizationException.GeneralErrors.INVALID_DISCOVERY_DOCUMENT))
        assertEquals(OAuthProblem.CONFIGURATION, appAuthProblem(AuthorizationException.TokenRequestErrors.INVALID_CLIENT))
        assertEquals(OAuthProblem.FAILED, appAuthProblem(null))
    }

    @Test fun descriptionsAndUnrecognizedErrorValuesCannotSelectCategoriesOrLeak() {
        val sentinel = "private-token-code-provider-body"
        val unknown = AuthorizationException(AuthorizationException.TYPE_OAUTH_TOKEN_ERROR, 999999,
            "unknown", "invalid_grant access_denied network $sentinel", null, IllegalStateException(sentinel))
        val safe = OAuthProblemException(appAuthProblem(unknown))
        assertEquals(OAuthProblem.FAILED, safe.problem)
        assertFalse(safe.toString().contains(sentinel))
        assertNull(safe.cause)
        val protocol = AuthorizationException(AuthorizationException.TYPE_OAUTH_TOKEN_ERROR, 999999,
            "invalid_grant", sentinel, null, null)
        assertEquals(OAuthProblem.RECONNECT, appAuthProblem(protocol))
        val wrongType = AuthorizationException(AuthorizationException.TYPE_GENERAL_ERROR, 999999,
            "invalid_grant", sentinel, null, null)
        assertEquals(OAuthProblem.FAILED, appAuthProblem(wrongType))
    }
}
