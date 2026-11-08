package com.trollm.app

import java.time.Instant

/** One installed credential generation. This value must never be logged. */
class MorayMachineCredential(
    val profileKey: String,
    val grantId: String,
    val tokenEndpoint: String,
    val clientId: String,
    val subject: String,
    val secret: String,
    val generation: String = grantId,
) {
    override fun toString(): String = "MorayMachineCredential(redacted)"
}

/** Authenticated issuer response, acquired without browser or refresh tokens. */
class MorayMachineAccess(val token: String, val expiresAt: Instant, val lifetimeSeconds: Long) {
    override fun toString(): String = "MorayMachineAccess(redacted)"
}

fun interface MorayMachineExchange {
    fun acquire(profile: MorayProfile, credential: MorayMachineCredential): MorayMachineAccess
}

/** The installed store owns selection. Token refresh never changes its grant identity.
 * Replacement/removal observed during a blocking exchange fences the original caller.
 * This is local credential currentness, not immediate server-side JWT revocation.
 */
class MorayMachineTokens(
    private val profile: MorayProfile,
    private val selected: () -> MorayMachineCredential?,
    private val exchange: MorayMachineExchange,
    private val now: () -> Instant = Instant::now,
    private val monotonicNanos: () -> Long = System::nanoTime,
) {
    private class Cached(val grantId: String, val access: MorayMachineAccess, val utc: Instant, val mono: Long)
    private var cached: Cached? = null
    private val generation = selected()?.generation
    private val acquisitionLock = java.util.concurrent.locks.ReentrantLock()

    fun pin(): String = current().grantId

    private fun current(expected: String? = null): MorayMachineCredential {
        val value = selected() ?: throw MorayFailure("SOURCE_MACHINE_PROVISIONING_REQUIRED")
        if (value.profileKey != profile.key || value.clientId != profile.clientId ||
            value.tokenEndpoint != profile.issuer + "/protocol/openid-connect/token" ||
            generation == null || value.generation != generation || value.generation.isBlank() ||
            value.grantId.isBlank() || value.subject.isBlank() || value.secret.isBlank() ||
            (expected != null && value.grantId != expected)) {
            throw MorayFailure("SOURCE_MACHINE_CREDENTIAL_CHANGED")
        }
        return value
    }

    fun accessToken(expectedGrantId: String): String {
        try { acquisitionLock.lockInterruptibly() }
        catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            throw kotlinx.coroutines.CancellationException("SOURCE_MACHINE_CANCELLED")
        }
        try { return accessLocked(expectedGrantId) }
        finally { acquisitionLock.unlock() }
    }

    private fun accessLocked(expectedGrantId: String): String {
        if (Thread.currentThread().isInterrupted) throw kotlinx.coroutines.CancellationException("SOURCE_MACHINE_CANCELLED")
        val credential = current(expectedGrantId)
        val utc = now()
        val mono = monotonicNanos()
        cached?.takeIf { it.grantId == expectedGrantId }?.let { retained ->
            val elapsed = mono - retained.mono
            if (elapsed < 0 || kotlin.math.abs(java.time.Duration.between(retained.utc, utc).toMillis() -
                    elapsed / 1_000_000) > 2_000) {
                cached = null
                throw MorayFailure("SOURCE_MACHINE_CLOCK_DISCONTINUITY")
            }
            // Both UTC and elapsed time must leave the same original expiry reserve.
            if (retained.access.expiresAt > utc.plusSeconds(15) &&
                elapsed / 1_000_000_000 < retained.access.lifetimeSeconds - 15) {
                current(expectedGrantId)
                return retained.access.token
            }
        }
        cached = null
        val started = now()
        val startedMono = monotonicNanos()
        val access = exchange.acquire(profile, credential)
        current(expectedGrantId)
        val finished = now()
        val elapsed = monotonicNanos() - startedMono
        if (elapsed < 0 || kotlin.math.abs(java.time.Duration.between(started, finished).toMillis() -
                elapsed / 1_000_000) > 2_000) throw MorayFailure("SOURCE_MACHINE_CLOCK_DISCONTINUITY")
        if (!ConnectionInput.validToken(access.token)) throw MorayFailure("SOURCE_MACHINE_TOKEN_ENCODING")
        if (access.lifetimeSeconds !in 16L..300L) throw MorayFailure("SOURCE_MACHINE_RESPONSE_LIFETIME")
        val originalCutoff = minOf(access.expiresAt, started.plusSeconds(access.lifetimeSeconds))
        if (originalCutoff <= finished.plusSeconds(15)) throw MorayFailure("SOURCE_MACHINE_EXPIRY_RESERVE")
        // Charge the entire original exchange RTT; never restart its age at receipt.
        cached = Cached(expectedGrantId,
            MorayMachineAccess(access.token, originalCutoff, access.lifetimeSeconds), started, startedMono)
        return access.token
    }
}
