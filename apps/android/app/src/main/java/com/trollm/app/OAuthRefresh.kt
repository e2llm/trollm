package com.trollm.app

import java.util.LinkedHashMap
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CancellationException

/** SDK adapter output contains secrets and must never acquire a data-class toString. */
class OAuthRefreshResult(
    val token: String?,
    val serializedState: String?,
    val problem: OAuthProblem? = null,
) {
    override fun toString() = "OAuthRefreshResult(redacted)"
}

/**
 * Worker-thread token access. execute must enqueue onto one serial persistence
 * executor; freshToken may complete asynchronously on any thread. SDK requests
 * stay coalesced through their callback even when an individual waiter times out.
 */
class OAuthRefresh(
    private val grants: OAuthGrants,
    private val execute: (() -> Unit) -> Unit,
    private val freshToken: (OAuthGrant, (OAuthRefreshResult) -> Unit) -> Unit,
    private val timeoutMillis: Long = 20_000,
) {
    private data class Key(val server: String, val endpoint: String, val grantId: String)
    private val lock = Any()
    private class Flight {
        val marker = Any()
        val pending = CompletableFuture<OAuthRefreshResult>()
    }
    // Only identity and a safe category survive a completed flight. In particular,
    // diagnostic history never retains its future, token, or serialized SDK state.
    private class Diagnostic(val marker: Any, val problem: OAuthProblem?)
    private val inFlight = mutableMapOf<Key, Flight>()
    private val diagnostics = LinkedHashMap<Key, Diagnostic>()

    init { require(timeoutMillis >= 0) }

    private fun key(server: String, endpoint: String, id: String): Key {
        require(ConnectionInput.validServer(server))
        return Key(server, ConnectionInput.canonicalEndpoint(endpoint), id)
    }

    private fun matching(key: Key) = grants.matching(key.server, key.grantId, key.endpoint)
        ?.takeIf { it.pendingRequest == null }

    private fun register(key: Key, flight: Flight) {
        diagnostics.remove(key)
        diagnostics[key] = Diagnostic(flight.marker, null)
        while (diagnostics.size > 32) diagnostics.remove(diagnostics.keys.first())
    }

    private fun record(key: Key, flight: Flight, problem: OAuthProblem?) {
        if (diagnostics[key]?.marker !== flight.marker) return
        diagnostics[key] = Diagnostic(flight.marker, problem)
    }

    private fun finish(key: Key, flight: Flight, result: OAuthRefreshResult) {
        synchronized(lock) {
            record(key, flight, result.problem)
            flight.pending.complete(result)
            if (inFlight[key] === flight) inFlight.remove(key)
        }
    }

    private fun refreshFailure(error: Throwable) = when (error) {
        is CancellationException -> OAuthProblem.CANCELLED
        is InterruptedException -> {
            Thread.currentThread().interrupt()
            OAuthProblem.CANCELLED
        }
        else -> classifyOAuthProblem(error)
    }

    private fun start(key: Key, flight: Flight) {
        try {
            execute {
                val grant = try { matching(key) } catch (_: Exception) {
                    finish(key, flight, OAuthRefreshResult(null, null, OAuthProblem.STORAGE))
                    return@execute
                }
                if (grant == null) {
                    finish(key, flight, OAuthRefreshResult(null, null, OAuthProblem.RECONNECT))
                    return@execute
                }
                val callbackDelivered = AtomicBoolean(false)
                try {
                    freshToken(grant) { result ->
                        if (callbackDelivered.compareAndSet(false, true)) {
                            try {
                                execute {
                                    val outcome = try {
                                        when {
                                            matching(key) == null -> OAuthRefreshResult(null, null, OAuthProblem.RECONNECT)
                                            result.serializedState != null && !grants.update(
                                                key.server, key.grantId, key.endpoint, result.serializedState,
                                            ) -> OAuthRefreshResult(null, null, OAuthProblem.RECONNECT)
                                            result.problem != null -> OAuthRefreshResult(null, null, result.problem)
                                            result.serializedState == null -> OAuthRefreshResult(null, null, OAuthProblem.FAILED)
                                            result.token == null || !ConnectionInput.validToken(result.token) ->
                                                OAuthRefreshResult(null, null, OAuthProblem.RECONNECT)
                                            else -> OAuthRefreshResult(result.token, null)
                                        }
                                    } catch (_: Exception) {
                                        OAuthRefreshResult(null, null, OAuthProblem.STORAGE)
                                    }
                                    finish(key, flight, outcome)
                                }
                            } catch (_: Exception) {
                                finish(key, flight, OAuthRefreshResult(null, null, OAuthProblem.STORAGE))
                            }
                        }
                    }
                } catch (error: Exception) {
                    if (callbackDelivered.compareAndSet(false, true)) {
                        finish(key, flight, OAuthRefreshResult(null, null, refreshFailure(error)))
                    }
                }
            }
        } catch (_: Exception) {
            finish(key, flight, OAuthRefreshResult(null, null, OAuthProblem.STORAGE))
        }
    }

    private fun <T> await(future: CompletableFuture<T>, started: Long): T {
        val remaining = TimeUnit.MILLISECONDS.toNanos(timeoutMillis) - (System.nanoTime() - started)
        return future.get(remaining.coerceAtLeast(0), TimeUnit.NANOSECONDS)
    }

    /** All credential-store reads, including this final identity check, are serialized. */
    private fun stillCurrent(key: Key, started: Long): Boolean {
        val checked = CompletableFuture<Boolean>()
        try {
            execute {
                try { checked.complete(matching(key) != null) }
                catch (_: Exception) { checked.completeExceptionally(OAuthProblemException(OAuthProblem.STORAGE)) }
            }
        } catch (_: Exception) {
            throw OAuthProblemException(OAuthProblem.STORAGE)
        }
        return await(checked, started)
    }

    fun accessTokenBlocking(server: String, endpoint: String, grantId: String): String? {
        val key = try { key(server, endpoint, grantId) } catch (_: Exception) { return null }
        var waitingForRefresh = true
        val started = System.nanoTime()
        var launch = false
        val flight = synchronized(lock) {
            inFlight[key] ?: Flight().also {
                inFlight[key] = it // before execute, including synchronous test adapters
                register(key, it)
                launch = true
            }
        }
        val pending = flight.pending
        if (launch) start(key, flight)
        return try {
            val result = await(pending, started)
            waitingForRefresh = false
            if (result.problem != null) return null
            if (!stillCurrent(key, started)) {
                synchronized(lock) { record(key, flight, OAuthProblem.RECONNECT) }
                null
            } else result.token
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            synchronized(lock) { if (!waitingForRefresh || !pending.isDone) record(key, flight, OAuthProblem.CANCELLED) }
            null
        } catch (_: TimeoutException) {
            synchronized(lock) { if (!waitingForRefresh || !pending.isDone) record(key, flight, OAuthProblem.TIMEOUT) }
            null
        } catch (error: ExecutionException) {
            val problem = (error.cause as? OAuthProblemException)?.problem ?: OAuthProblem.FAILED
            synchronized(lock) { record(key, flight, problem) }
            null
        } catch (error: Exception) {
            synchronized(lock) { record(key, flight, refreshFailure(error)) }
            null
        }
    }

    /** Only the currently selected exact grant may expose its process-local problem. */
    fun problem(server: String, endpoint: String, grantId: String): OAuthProblem? {
        val key = try { key(server, endpoint, grantId) } catch (_: Exception) { return OAuthProblem.CONFIGURATION }
        return try {
            if (stillCurrent(key, System.nanoTime())) synchronized(lock) { diagnostics[key]?.problem } else null
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            OAuthProblem.CANCELLED
        } catch (_: TimeoutException) { OAuthProblem.TIMEOUT }
        catch (_: Exception) { OAuthProblem.STORAGE }
    }

    fun clearServer(server: String) {
        synchronized(lock) { diagnostics.keys.removeAll { it.server == server } }
    }
}
