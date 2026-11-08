package com.trollm.app

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.interfaces.RSAPublicKey
import java.time.Instant
import java.util.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.withTimeout

/** Real JCA signatures with declared credentials; no issuer/account/network call. */
class MorayMachineClaimsTest {
    private val profile = MorayProfile("https://source.test/v1/portable-publications",
        "https://issuer.test/realms/source", "installed-client", "a".repeat(40))
    private val now = Instant.parse("2026-11-07T00:00:00Z")
    private fun claims() = JSONObject().put("iss", profile.issuer).put("azp", profile.clientId)
        .put("sub", "declared-service-subject").put("aud", "urn:moray:portable").put("typ", "Bearer")
        .put("scope", "moray:bundle:read moray:edge-report:write")
        .put("iat", now.epochSecond).put("exp", now.epochSecond + 300)
    private fun encode(bytes: ByteArray) = Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    private fun keyBytes(value: java.math.BigInteger): ByteArray = value.toByteArray().let {
        if (it[0] == 0.toByte()) it.copyOfRange(1, it.size) else it
    }
    private fun jwks(): JSONObject {
        val public = pair.public as RSAPublicKey
        return JSONObject().put("keys", JSONArray().put(JSONObject().put("kid", "declared-key")
            .put("kty", "RSA").put("use", "sig").put("alg", "RS256")
            .put("n", encode(keyBytes(public.modulus))).put("e", encode(keyBytes(public.publicExponent)))))
    }
    private fun token(value: JSONObject, alg: String = "RS256"): String {
        val head = encode(JSONObject().put("alg", alg).put("typ", "JWT").put("kid", "declared-key").toString().toByteArray())
        val body = head + "." + encode(value.toString().toByteArray())
        val signature = Signature.getInstance("SHA256withRSA").apply {
            initSign(pair.private); update(body.toByteArray())
        }.sign()
        return body + "." + encode(signature)
    }
    private fun verify(value: String, keys: JSONObject = jwks()) =
        MorayMachineClaims.verify(value, keys, profile, "declared-service-subject", now)
    private fun reject(block: () -> Unit) {
        try { block(); fail("accepted an invalid machine token") }
        catch (_: IllegalArgumentException) {}
    }

    @Test fun actualSignatureAndExactIdentityScopeLifetimeAreRequired() {
        assertEquals(now.plusSeconds(300), verify(token(claims())))
        val variants = listOf(
            claims().put("iss", "https://other.test/realms/source"),
            claims().put("azp", "other-client"), claims().put("sub", "other-principal"),
            claims().put("aud", JSONArray().put("urn:moray:portable")),
            claims().put("aud", "other-audience"), claims().put("typ", "ID"),
            claims().put("scope", "moray:bundle:read"),
            claims().put("scope", "moray:bundle:read moray:edge-report:write extra"),
            claims().put("scope", "moray:bundle:read moray:bundle:read"),
            claims().put("iat", now.epochSecond + 1), claims().put("exp", now.epochSecond + 301),
            claims().put("exp", now.epochSecond), claims().put("nbf", now.epochSecond + 1),
            claims().put("exp", "300"),
        )
        for (value in variants) reject { verify(token(value)) }
        reject { verify(token(claims(), "HS256")) }
        val original = token(claims())
        val fields = original.split('.')
        reject { verify(fields[0] + "." + encode(claims().put("sub", "changed").toString().toByteArray()) + "." + fields[2]) }
        val duplicate = jwks().apply { getJSONArray("keys").put(getJSONArray("keys").getJSONObject(0)) }
        reject { verify(original, duplicate) }
        reject { verify(original, jwks().apply { getJSONArray("keys").getJSONObject(0).put("kid", "unselected") }) }
    }

    @Test fun signedClaimFailuresExposeOnlyClosedStageAndKeepStrictTimeBoundaries() {
        fun code(value: JSONObject, observed: Instant): String {
            try { MorayMachineClaims.verify(token(value), jwks(), profile, "declared-service-subject", observed)
                fail("invalid claims accepted") }
            catch (failure: MorayMachineClaimFailure) {
                assertEquals(failure.code, failure.message)
                assertNull(failure.cause)
                assertFalse(failure.message!!.contains("declared-service-subject"))
                return failure.code
            }
            error("unreachable")
        }
        assertEquals("SOURCE_MACHINE_CLAIMS_ISSUED_FUTURE", code(claims(), now.minusMillis(1)))
        assertEquals("SOURCE_MACHINE_CLAIMS_ISSUED_FUTURE", code(claims(), now.minusMillis(900)))
        assertEquals(now.plusSeconds(300), MorayMachineClaims.verify(token(claims()), jwks(), profile,
            "declared-service-subject", now.plusMillis(1)))
        assertEquals("SOURCE_MACHINE_CLAIMS_NOT_BEFORE", code(claims().put("nbf", now.epochSecond + 1), now))
        assertEquals("SOURCE_MACHINE_CLAIMS_EXPIRY_RESERVE", code(claims(), now.plusSeconds(285)))
        assertEquals("SOURCE_MACHINE_CLAIMS_LIFETIME", code(claims().put("exp", now.epochSecond + 301), now))
        assertEquals("SOURCE_MACHINE_CLAIMS_IDENTITY", code(claims().put("sub", "foreign-subject"), now))
    }

    private class Connection(url: String, private val status: Int, private val response: ByteArray) :
        javax.net.ssl.HttpsURLConnection(java.net.URL(url)) {
        val sent = java.io.ByteArrayOutputStream()
        @Volatile var disconnected = false
        var outputFailure: java.io.IOException? = null
        @Volatile var responseStream: java.io.InputStream? = null
        override fun connect() {}
        override fun usingProxy() = false
        override fun disconnect() { disconnected = true; responseStream?.close() }
        override fun getCipherSuite() = "declared-no-TLS-handshake"
        override fun getLocalCertificates(): Array<java.security.cert.Certificate>? = null
        override fun getServerCertificates(): Array<java.security.cert.Certificate> = emptyArray()
        override fun getOutputStream(): java.io.OutputStream {
            outputFailure?.let { throw it }
            return sent
        }
        override fun getInputStream(): java.io.InputStream = responseStream ?: response.inputStream()
        override fun getResponseCode() = status
    }

    @Test fun actualFormAndRequestOptionsDoNotFollowRedirectOrForwardCredentialsToJwks() {
        val directory = java.nio.file.Files.createTempDirectory("machine-exchange").toFile()
        try {
            val response = JSONObject().put("access_token", token(claims())).put("token_type", "Bearer")
                .put("expires_in", 300).toString().toByteArray()
            val requests = mutableListOf<Connection>()
            val exchange = MorayMachineHttps(directory, { now }) { url ->
                Connection(url, 200, if (url.endsWith("/token")) response else jwks().toString().toByteArray())
                    .also { requests.add(it) }
            }
            val credential = MorayMachineCredential(profile.key, "lineage", profile.issuer +
                "/protocol/openid-connect/token", profile.clientId, "declared-service-subject", "declared+secret&value")
            assertEquals(now.plusSeconds(300), exchange.acquire(profile, credential).expiresAt)
            assertEquals(2, requests.size)
            val post = requests.first()
            assertEquals("POST", post.requestMethod)
            assertEquals("application/x-www-form-urlencoded", post.getRequestProperty("Content-Type"))
            assertTrue(post.sent.toString("UTF-8").contains("client_secret=declared%2Bsecret%26value"))
            assertTrue(post.sent.toString("UTF-8").contains("grant_type=client_credentials"))
            assertEquals("GET", requests.last().requestMethod)
            assertEquals(0, requests.last().sent.size())
            for (request in requests) {
                assertFalse(request.instanceFollowRedirects)
                assertEquals(15_000, request.connectTimeout)
                assertEquals(15_000, request.readTimeout)
                assertNull(request.getRequestProperty("Authorization"))
                assertTrue(request.disconnected)
            }
        } finally { directory.deleteRecursively() }
    }

    @Test fun signedFailureLeavesWarmProviderRecoverableWithoutChangingGrantOrTimePolicy() {
        val directory = java.nio.file.Files.createTempDirectory("machine-warm-cold").toFile()
        try {
            var utc = now.minusMillis(900)
            var mono = 0L
            var requests = 0
            val response = JSONObject().put("access_token", token(claims())).put("token_type", "Bearer")
                .put("expires_in", 300).toString().toByteArray()
            val exchange = MorayMachineHttps(directory, { utc }) { url ->
                requests++; Connection(url, 200, if (url.endsWith("/token")) response else jwks().toString().toByteArray())
            }
            val credential = MorayMachineCredential(profile.key, "original-lineage", profile.issuer +
                "/protocol/openid-connect/token", profile.clientId, "declared-service-subject", "declared-secret")
            fun provider() = MorayMachineTokens(profile, { credential }, exchange, { utc }, { mono })
            val warm = provider()
            try { warm.accessToken(warm.pin()); fail("future token admitted") }
            catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_CLAIMS_ISSUED_FUTURE", failure.code) }
            assertEquals(2, requests)
            utc = now; mono = 900_000_000
            val token = warm.accessToken(warm.pin())
            assertEquals(4, requests)
            assertEquals(token, warm.accessToken(warm.pin()))
            assertEquals(4, requests)
            val cold = provider()
            assertEquals(warm.pin(), cold.pin())
            assertEquals(token, cold.accessToken(cold.pin()))
            assertEquals(6, requests)
            // Declared control only, not attribution of the retired guest's
            // warm accurate-UTC failure followed by cold-start success.
        } finally { directory.deleteRecursively() }
    }

    @Test fun redirectsUnauthorizedAndTlsFailureHaveClosedCauseAndNeverRetry() {
        val directory = java.nio.file.Files.createTempDirectory("machine-refusal").toFile()
        val credential = MorayMachineCredential(profile.key, "lineage", profile.issuer +
            "/protocol/openid-connect/token", profile.clientId, "declared-service-subject", "declared-secret")
        try {
            for (status in listOf(302, 401, 403, 503)) {
                var calls = 0
                val exchange = MorayMachineHttps(directory, { now }) { url ->
                    calls++; Connection(url, status, byteArrayOf())
                }
                try { exchange.acquire(profile, credential); fail("unexpected issuer response accepted") }
                catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_HTTP_$status", failure.code) }
                assertEquals(1, calls)
            }
            var calls = 0
            val tls = MorayMachineHttps(directory, { now }) { url ->
                calls++; Connection(url, 200, byteArrayOf()).apply {
                    outputFailure = javax.net.ssl.SSLHandshakeException("declared private diagnostic")
                }
            }
            try { tls.acquire(profile, credential); fail("TLS failure accepted") }
            catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_TLS_FAILED", failure.code) }
            assertEquals(1, calls)
        } finally { directory.deleteRecursively() }
    }

    private class ProgressStream(bytes: ByteArray, private val pauseMillis: Long, private val chunk: Int) :
        java.io.ByteArrayInputStream(bytes) {
        val progress = java.util.concurrent.atomic.AtomicInteger()
        private val closed = java.util.concurrent.atomic.AtomicBoolean()
        override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
            if (available() == 0) return -1
            try { Thread.sleep(pauseMillis) } catch (_: InterruptedException) { /* close, not interrupt, ends the input */ }
            if (closed.get()) throw java.io.IOException("declared disconnected response")
            val count = super.read(buffer, offset, minOf(length, chunk))
            if (count > 0) progress.incrementAndGet()
            return count
        }
        override fun close() { closed.set(true) }
    }

    @Test fun continuouslyProgressingBodyCannotResetOriginalTotalDeadline() {
        val directory = java.nio.file.Files.createTempDirectory("machine-slow-progress").toFile()
        val response = JSONObject().put("access_token", token(claims())).put("token_type", "Bearer")
            .put("expires_in", 300).toString().toByteArray()
        val stream = ProgressStream(response, 10, 1)
        val calls = mutableListOf<Connection>()
        val credential = MorayMachineCredential(profile.key, "lineage", profile.issuer +
            "/protocol/openid-connect/token", profile.clientId, "declared-service-subject", "declared-secret")
        val exchange = MorayMachineHttps(directory, { now }, totalBudgetMillis = 500) { url ->
            Connection(url, 200, response).apply { responseStream = stream }.also { calls.add(it) }
        }
        val started = System.nanoTime()
        try {
            try { exchange.acquire(profile, credential); fail("slow progress escaped the total budget") }
            catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_TIMEOUT", failure.code) }
            assertTrue(java.util.concurrent.TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started) < 3_000)
            assertTrue("the intended continuing-progress boundary was reached", stream.progress.get() > 1)
            assertEquals(1, calls.size)
            assertTrue(calls.single().disconnected)
        } finally { stream.close(); directory.deleteRecursively() }
    }

    @Test fun jwksSharesPostAnchorAndCancellationDisconnectsAnInterruptIgnoringRead() {
        val directory = java.nio.file.Files.createTempDirectory("machine-shared-budget").toFile()
        val response = JSONObject().put("access_token", token(claims())).put("token_type", "Bearer")
            .put("expires_in", 300).toString().toByteArray()
        val requests = mutableListOf<Connection>()
        val credential = MorayMachineCredential(profile.key, "lineage", profile.issuer +
            "/protocol/openid-connect/token", profile.clientId, "declared-service-subject", "declared-secret")
        try {
            val exchange = MorayMachineHttps(directory, { now }, totalBudgetMillis = 500) { url ->
                Connection(url, 200, response).apply {
                    responseStream = if (url.endsWith("/token")) ProgressStream(response, 150, response.size)
                        else ProgressStream(jwks().toString().toByteArray(), 10, 1)
                }.also { requests.add(it) }
            }
            try { exchange.acquire(profile, credential); fail("JWKS renewed the POST budget") }
            catch (failure: MorayFailure) { assertEquals("SOURCE_MACHINE_TIMEOUT", failure.code) }
            assertEquals(2, requests.size)
            assertTrue(requests.last().readTimeout < requests.first().readTimeout - 100)
            assertTrue((requests.last().responseStream as ProgressStream).progress.get() > 1)
            assertTrue(requests.all { it.disconnected })

            val entered = java.util.concurrent.CountDownLatch(1)
            val closed = java.util.concurrent.CountDownLatch(1)
            val readExited = java.util.concurrent.CountDownLatch(1)
            val blocked = object : java.io.InputStream() {
                override fun read(): Int = throw UnsupportedOperationException()
                override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                    entered.countDown()
                    try {
                        while (closed.count > 0) {
                            try { closed.await() } catch (_: InterruptedException) { /* actual disconnect required */ }
                        }
                        throw java.io.IOException("declared disconnected response")
                    } finally { readExited.countDown() }
                }
                override fun close() { closed.countDown() }
            }
            val connection = Connection(credential.tokenEndpoint, 200, response).apply { responseStream = blocked }
            val cancelExchange = MorayMachineHttps(directory, { now }) { connection }
            val outcome = java.util.concurrent.atomic.AtomicReference<Throwable?>()
            val caller = Thread {
                try { cancelExchange.acquire(profile, credential); outcome.set(AssertionError("cancel returned token")) }
                catch (failure: Throwable) { outcome.set(failure) }
            }
            try {
                caller.start()
                assertTrue(entered.await(3, java.util.concurrent.TimeUnit.SECONDS))
                caller.interrupt(); caller.join(3_000)
                assertFalse("the cancelled caller must return", caller.isAlive)
                assertTrue(outcome.get() is kotlinx.coroutines.CancellationException)
                assertTrue(connection.disconnected)
                assertEquals("the actual blocked read exited before return", 0L, readExited.count)
            } finally { blocked.close(); caller.interrupt(); caller.join(3_000) }
        } finally { directory.deleteRecursively() }
    }

    @Test fun realCoroutineCancellationReachesBlockingAcquisitionAndRetiresItsRead() = runBlocking {
        val directory = java.nio.file.Files.createTempDirectory("machine-coroutine-cancel").toFile()
        val entered = java.util.concurrent.CountDownLatch(1)
        val closed = java.util.concurrent.CountDownLatch(1)
        val exited = java.util.concurrent.CountDownLatch(1)
        val sawCancellation = java.util.concurrent.atomic.AtomicBoolean()
        val blocked = object : java.io.InputStream() {
            override fun read(): Int = throw UnsupportedOperationException()
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                entered.countDown()
                try {
                    while (closed.count > 0) {
                        try { closed.await() } catch (_: InterruptedException) { /* disconnect must release this read */ }
                    }
                    throw java.io.IOException("declared closed stream")
                } finally { exited.countDown() }
            }
            override fun close() { closed.countDown() }
        }
        val credential = MorayMachineCredential(profile.key, "lineage", profile.issuer +
            "/protocol/openid-connect/token", profile.clientId, "declared-service-subject", "declared-secret")
        val connection = Connection(credential.tokenEndpoint, 200, byteArrayOf()).apply { responseStream = blocked }
        val exchange = MorayMachineHttps(directory, { now }) { connection }
        val job = launch(Dispatchers.IO) {
            try { runInterruptible { exchange.acquire(profile, credential) }; fail("cancel returned access") }
            catch (cancelled: kotlinx.coroutines.CancellationException) { sawCancellation.set(true); throw cancelled }
        }
        try {
            assertTrue(entered.await(3, java.util.concurrent.TimeUnit.SECONDS))
            job.cancel()
            withTimeout(3_000) { job.join() }
            assertTrue(job.isCancelled)
            assertTrue(sawCancellation.get())
            assertTrue(connection.disconnected)
            assertEquals(0L, exited.count)
        } finally {
            blocked.close(); job.cancel()
            withTimeout(3_000) { job.join() }
            directory.deleteRecursively()
        }
    }

    companion object {
        private val pair by lazy { KeyPairGenerator.getInstance("RSA").apply { initialize(2048) }.generateKeyPair() }
    }
}
