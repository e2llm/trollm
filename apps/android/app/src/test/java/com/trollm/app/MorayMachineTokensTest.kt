package com.trollm.app

import org.junit.Assert.*
import org.junit.Test
import java.time.Instant

class MorayMachineTokensTest {
    private val profile = MorayProfile("https://source.test/v1/portable-publications",
        "https://issuer.test/realms/source", "installed-client", "a".repeat(40))
    private val start = Instant.parse("2026-11-07T00:00:00Z")
    private fun credential(id: String = "generation-one") = MorayMachineCredential(
        profile.key, id, profile.issuer + "/protocol/openid-connect/token", profile.clientId,
        "dedicated-service-subject", "declared-test-secret")
    private fun refuses(code: String, block: () -> Unit) {
        try { block(); fail("accepted invalid machine authority") }
        catch (failure: MorayFailure) { assertEquals(code, failure.code) }
    }

    @Test fun refreshPreservesOriginalGrantAndChargesOriginalLifetime() {
        var utc = start
        var mono = 0L
        var exchanges = 0
        val provider = MorayMachineTokens(profile, { credential() }, MorayMachineExchange { _, _ ->
            exchanges++
            MorayMachineAccess("token-$exchanges", utc.plusSeconds(300), 300)
        }, { utc }, { mono })
        val grant = provider.pin()
        assertEquals("token-1", provider.accessToken(grant))
        utc = utc.plusSeconds(280); mono += 280_000_000_000
        assertEquals("token-1", provider.accessToken(grant))
        utc = utc.plusSeconds(6); mono += 6_000_000_000
        assertEquals("token-2", provider.accessToken(grant))
        assertEquals(grant, provider.pin())
        assertEquals(2, exchanges)
    }

    @Test fun removalAndReplacementDuringIssuerWaitCannotReturnOriginalToken() {
        for (replacement in listOf<MorayMachineCredential?>(null, credential("generation-two"))) {
            var selected: MorayMachineCredential? = credential()
            val provider = MorayMachineTokens(profile, { selected }, MorayMachineExchange { _, _ ->
                selected = replacement
                MorayMachineAccess("declared-access", start.plusSeconds(300), 300)
            }, { start }, { 0L })
            refuses(if (replacement == null) "SOURCE_MACHINE_PROVISIONING_REQUIRED" else
                "SOURCE_MACHINE_CREDENTIAL_CHANGED") { provider.accessToken("generation-one") }
        }
    }

    @Test fun replacementNeverReusesPreviousGenerationCache() {
        var selected = credential()
        var exchanges = 0
        val provider = MorayMachineTokens(profile, { selected }, MorayMachineExchange { _, value ->
            exchanges++
            MorayMachineAccess(value.grantId, start.plusSeconds(300), 300)
        }, { start }, { 0L })
        assertEquals("generation-one", provider.accessToken(provider.pin()))
        selected = credential("generation-two")
        refuses("SOURCE_MACHINE_CREDENTIAL_CHANGED") { provider.accessToken("generation-one") }
        refuses("SOURCE_MACHINE_CREDENTIAL_CHANGED") { provider.pin() }
        assertEquals(1, exchanges)
    }

    @Test fun clockDiscontinuityAndExchangeThatExhaustsExpiryRefuse() {
        var utc = start
        var mono = 0L
        val provider = MorayMachineTokens(profile, { credential() }, MorayMachineExchange { _, _ ->
            MorayMachineAccess("declared-access", start.plusSeconds(300), 300)
        }, { utc }, { mono })
        provider.accessToken(provider.pin())
        utc = start.plusSeconds(10)
        refuses("SOURCE_MACHINE_CLOCK_DISCONTINUITY") { provider.accessToken(provider.pin()) }
        utc = start
        val slow = MorayMachineTokens(profile, { credential() }, MorayMachineExchange { _, _ ->
            utc = start.plusSeconds(290); mono = 290_000_000_000
            MorayMachineAccess("declared-access", start.plusSeconds(300), 300)
        }, { utc }, { mono })
        refuses("SOURCE_MACHINE_EXPIRY_RESERVE") { slow.accessToken(slow.pin()) }
    }

    @Test fun samePrincipalCredentialRotationKeepsGrantButFencesOriginalGeneration() {
        var selected = credential()
        val exchange = MorayMachineExchange { _, value ->
            MorayMachineAccess("access-${value.generation}", start.plusSeconds(300), 300)
        }
        val original = MorayMachineTokens(profile, { selected }, exchange, { start }, { 0L })
        val grant = original.pin()
        original.accessToken(grant)
        selected = MorayMachineCredential(profile.key, grant, selected.tokenEndpoint, profile.clientId,
            selected.subject, "rotated-declared-secret", "generation-two")
        refuses("SOURCE_MACHINE_CREDENTIAL_CHANGED") { original.accessToken(grant) }
        val current = MorayMachineTokens(profile, { selected }, exchange, { start }, { 0L })
        assertEquals(grant, current.pin())
        assertEquals("access-generation-two", current.accessToken(grant))
    }

    @Test fun concurrentConsumersShareOneBlockedIssuerAcquisition() {
        val entered = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val exchanges = java.util.concurrent.atomic.AtomicInteger()
        val provider = MorayMachineTokens(profile, { credential() }, MorayMachineExchange { _, _ ->
            exchanges.incrementAndGet(); entered.countDown()
            check(release.await(5, java.util.concurrent.TimeUnit.SECONDS))
            MorayMachineAccess("declared-access", start.plusSeconds(300), 300)
        }, { start }, { 0L })
        val pool = java.util.concurrent.Executors.newFixedThreadPool(2)
        try {
            val first = pool.submit<String> { provider.accessToken("generation-one") }
            assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS))
            val second = pool.submit<String> { provider.accessToken("generation-one") }
            release.countDown()
            assertEquals("declared-access", first.get(5, java.util.concurrent.TimeUnit.SECONDS))
            assertEquals("declared-access", second.get(5, java.util.concurrent.TimeUnit.SECONDS))
            assertEquals(1, exchanges.get())
        } finally {
            release.countDown(); pool.shutdownNow()
            assertTrue(pool.awaitTermination(5, java.util.concurrent.TimeUnit.SECONDS))
        }
    }

    @Test fun delayedIssuerIssuanceAttenuatesCacheFromOriginalRequestRatherThanRejectingValidToken() {
        var utc = start
        var mono = 0L
        var exchanges = 0
        val provider = MorayMachineTokens(profile, { credential() }, MorayMachineExchange { _, _ ->
            exchanges++; utc = utc.plusSeconds(20); mono += 20_000_000_000
            MorayMachineAccess("access-$exchanges", utc.plusSeconds(300), 300)
        }, { utc }, { mono })
        assertEquals("access-1", provider.accessToken("generation-one"))
        utc = start.plusSeconds(286); mono = 286_000_000_000
        assertEquals("access-2", provider.accessToken("generation-one"))
        assertEquals(2, exchanges)
    }

    @Test fun cancelledConsumerDoesNotWaitForAnotherConsumersProviderLock() {
        val entered = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val calls = java.util.concurrent.atomic.AtomicInteger()
        val provider = MorayMachineTokens(profile, { credential() }, MorayMachineExchange { _, _ ->
            calls.incrementAndGet(); entered.countDown()
            check(release.await(5, java.util.concurrent.TimeUnit.SECONDS))
            MorayMachineAccess("declared-access", start.plusSeconds(300), 300)
        }, { start }, { 0L })
        val firstFailure = java.util.concurrent.atomic.AtomicReference<Throwable?>()
        val cancelled = java.util.concurrent.atomic.AtomicReference<Throwable?>()
        val first = Thread {
            try { provider.accessToken("generation-one") } catch (failure: Throwable) { firstFailure.set(failure) }
        }
        val waiting = Thread {
            try { provider.accessToken("generation-one"); cancelled.set(AssertionError("cancel returned access")) }
            catch (failure: Throwable) { cancelled.set(failure) }
        }
        try {
            first.start(); assertTrue(entered.await(3, java.util.concurrent.TimeUnit.SECONDS))
            waiting.start(); waiting.interrupt(); waiting.join(2_000)
            assertFalse(waiting.isAlive)
            assertTrue(cancelled.get() is kotlinx.coroutines.CancellationException)
            assertTrue(first.isAlive)
            assertEquals(1, calls.get())
        } finally {
            release.countDown(); first.join(3_000); waiting.interrupt(); waiting.join(3_000)
            assertFalse(first.isAlive); assertFalse(waiting.isAlive)
        }
        assertNull(firstFailure.get())
    }
}
