package com.trollm.app

import java.io.IOException
import java.net.SocketTimeoutException
import java.util.concurrent.TimeoutException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext

/** Fixed, process-local diagnostics. Never include SDK responses or credential text. */
enum class OAuthProblem(val message: String) {
    NETWORK("OAuth could not reach the sign-in server. Check the connection and try again."),
    TIMEOUT("OAuth timed out. Try again."),
    CANCELLED("OAuth was cancelled."),
    RECONNECT("Sign in again to reconnect this account."),
    STORAGE("OAuth credentials could not be saved or read. Try again."),
    CONFIGURATION("OAuth configuration is unavailable or invalid."),
    FAILED("OAuth could not finish. Try again."),
}

/** No raw exception cause, response body, state, endpoint or token is retained. */
class OAuthProblemException(val problem: OAuthProblem) : Exception(problem.message)

fun classifyOAuthProblem(error: Throwable): OAuthProblem = when (error) {
    is CancellationException -> throw error
    is OAuthProblemException -> error.problem
    is SocketTimeoutException, is TimeoutException -> OAuthProblem.TIMEOUT
    is InterruptedException -> OAuthProblem.CANCELLED
    is IOException -> OAuthProblem.NETWORK
    else -> OAuthProblem.FAILED
}

private fun safeOAuthFailure(error: Exception): Exception =
    if (error is CancellationException) error else OAuthProblemException(classifyOAuthProblem(error))

/** Cleanup cannot replace a primary cancellation or expose a storage exception. */
private suspend fun cleanOAuth(
    primary: Exception?,
    dispatcher: CoroutineDispatcher,
    cleanup: () -> Unit,
) {
    try {
        withContext(NonCancellable + dispatcher) { cleanup() }
    } catch (cancelled: CancellationException) {
        // Cleanup itself is protected, but delivery back to the caller can be
        // cancelled after its side effects completed. This is not storage IO.
        throw (primary as? CancellationException ?: cancelled)
    } catch (_: Exception) {
        val safe = OAuthProblemException(OAuthProblem.STORAGE)
        if (primary == null) throw safe
        primary.addSuppressed(safe)
    }
}

/** Complete/exchange callers supply an exact-grant, pending-only cleanup action. */
suspend fun <T> withOAuthCleanup(
    dispatcher: CoroutineDispatcher = Dispatchers.IO,
    cleanup: () -> Unit,
    action: suspend () -> T,
): T {
    var primary: Exception? = null
    try {
        return action()
    } catch (error: Exception) {
        val safe = safeOAuthFailure(error)
        primary = safe
        throw safe
    } finally {
        cleanOAuth(primary, dispatcher, cleanup)
    }
}

/** Capture ownership on the committing dispatcher, before cancellation at delivery. */
suspend fun <T> withPendingOAuthGrant(
    begin: () -> OAuthGrant,
    invalidate: (OAuthGrant) -> Unit,
    dispatcher: CoroutineDispatcher = Dispatchers.IO,
    action: suspend (OAuthGrant) -> T,
): T {
    var owned: OAuthGrant? = null
    try {
        val grant = withContext(dispatcher) { begin().also { owned = it } }
        return action(grant)
    } catch (error: Exception) {
        val safe = safeOAuthFailure(error)
        cleanOAuth(safe, dispatcher) { owned?.let(invalidate) }
        throw safe
    }
}
