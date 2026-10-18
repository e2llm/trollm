package com.trollm.app

import java.io.IOException
import java.net.SocketTimeoutException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.junit.Assert.*
import org.junit.Test

class OAuthOperationsTest {
    private fun grants(): OAuthGrants {
        val values = mutableMapOf<String, StoredConnection>()
        return OAuthGrants({ values[it] ?: StoredConnection(null, null, null) },
            { name, value -> values[name] = value }, Any())
    }

    @Test fun cancellationAfterCommitBeforeDispatcherDeliveryCleansOwnedPendingGrant() = runBlocking {
        val grants = grants()
        val committed = CountDownLatch(1)
        val release = CountDownLatch(1)
        val actionCalled = AtomicBoolean(false)
        val dispatcher = Executors.newSingleThreadExecutor().asCoroutineDispatcher()
        try {
            val worker = launch(Dispatchers.Default) {
                withPendingOAuthGrant(
                    begin = {
                        val grant = grants.begin("browser", "https://example.test/mcp", "private-state", "request")
                        committed.countDown()
                        check(release.await(5, TimeUnit.SECONDS))
                        grant
                    },
                    invalidate = { grants.invalidatePending(it.server, it.id) },
                    dispatcher = dispatcher,
                ) { actionCalled.set(true) }
            }
            try {
                assertTrue(committed.await(5, TimeUnit.SECONDS))
                worker.cancel()
            } finally { release.countDown() }
            worker.join()
            assertFalse(actionCalled.get())
            assertNull(grants.current("browser"))
            assertEquals(SelectedAuthMode.UNAVAILABLE, grants.mode("browser"))
        } finally { dispatcher.close() }
    }

    @Test fun failureCleanupCannotInvalidateReplacementOrCompletedSameGrant() = runBlocking {
        for (replacement in listOf(false, true)) {
            val grants = grants()
            var expectedId = ""
            try {
                withPendingOAuthGrant(
                    begin = { grants.begin("browser", "https://example.test/mcp", "state", "request") },
                    invalidate = { grants.invalidatePending(it.server, it.id) },
                ) { grant ->
                    expectedId = if (replacement) {
                        grants.begin("browser", grant.endpoint, "replacement", "new-request").id
                    } else {
                        assertTrue(grants.update("browser", grant.id, grant.endpoint, "authorized"))
                        grant.id
                    }
                    throw IOException("private-token-state")
                }
                fail("failure expected")
            } catch (error: OAuthProblemException) {
                assertEquals(OAuthProblem.NETWORK, error.problem)
                assertNull(error.cause)
                assertFalse(error.stackTraceToString().contains("private-token-state"))
            }
            assertEquals(expectedId, grants.current("browser")?.id)
        }
    }

    @Test fun completionCancellationAtIoDeliveryCleansPendingButPreservesCompletedGrant() = runBlocking {
        for (completed in listOf(false, true)) {
            val grants = grants()
            val grant = grants.begin("browser", "https://example.test/mcp", "pending", "request")
            val committed = CountDownLatch(1)
            val release = CountDownLatch(1)
            val dispatcher = Executors.newSingleThreadExecutor().asCoroutineDispatcher()
            try {
                val worker = launch(Dispatchers.Default) {
                    withOAuthCleanup(
                        dispatcher = dispatcher,
                        cleanup = { grants.invalidatePending(grant.server, grant.id) },
                    ) {
                        withContext(dispatcher) {
                            if (completed) check(grants.update(grant.server, grant.id, grant.endpoint, "authorized"))
                            else check(grants.current(grant.server)?.id == grant.id)
                            committed.countDown()
                            check(release.await(5, TimeUnit.SECONDS))
                        }
                    }
                }
                try {
                    assertTrue(committed.await(5, TimeUnit.SECONDS))
                    worker.cancel()
                } finally { release.countDown() }
                worker.join()
                if (completed) {
                    assertEquals(grant.id, grants.current(grant.server)?.id)
                    assertEquals("authorized", grants.current(grant.server)?.state)
                    assertNotNull(grants.pin(grant.server, grant.endpoint))
                } else {
                    assertNull(grants.current(grant.server))
                    assertEquals(SelectedAuthMode.UNAVAILABLE, grants.mode(grant.server))
                }
            } finally { dispatcher.close() }
        }
    }

    @Test fun cancellationDuringCleanupAfterSuccessfulActionRemainsCancellation() = runBlocking {
        val cleanupStarted = CountDownLatch(1)
        val release = CountDownLatch(1)
        val actionSucceeded = AtomicBoolean(false)
        val cleanupFinished = AtomicBoolean(false)
        val observed = AtomicReference<Exception?>()
        val dispatcher = Executors.newSingleThreadExecutor().asCoroutineDispatcher()
        try {
            val worker = launch(Dispatchers.Default) {
                try {
                    withOAuthCleanup(
                        dispatcher = dispatcher,
                        cleanup = {
                            cleanupStarted.countDown()
                            release.await()
                            cleanupFinished.set(true)
                        },
                    ) { actionSucceeded.set(true) }
                } catch (error: Exception) {
                    // Observe inside the cancelled coroutine: its Job would be
                    // cancelled even if the helper incorrectly threw STORAGE.
                    observed.set(error)
                }
            }
            try {
                assertTrue(cleanupStarted.await(5, TimeUnit.SECONDS))
                assertTrue(actionSucceeded.get())
                worker.cancel()
            } finally { release.countDown() }
            worker.join()
            assertTrue(cleanupFinished.get())
            assertTrue("cleanup delivery cancellation was converted to storage failure",
                observed.get() is CancellationException)
        } finally { dispatcher.close() }
    }

    @Test fun cleanupPreservesPrimaryCancellationAndOnlyAddsSanitizedStorageProblem() = runBlocking {
        val cancelled = CancellationException("cancelled by caller")
        try {
            withOAuthCleanup(cleanup = { throw IOException("private-secret") }) { throw cancelled }
            fail("cancellation expected")
        } catch (error: CancellationException) {
            assertSame(cancelled, error)
            assertEquals(1, error.suppressed.size)
            val secondary = error.suppressed.single() as OAuthProblemException
            assertEquals(OAuthProblem.STORAGE, secondary.problem)
            assertNull(secondary.cause)
            assertFalse(error.stackTraceToString().contains("private-secret"))
        }
    }

    @Test fun standaloneCleanupAndBeginFailuresAreSanitized() = runBlocking {
        try {
            withOAuthCleanup(cleanup = { throw IOException("private-cleanup") }) { true }
            fail("storage failure expected")
        } catch (error: OAuthProblemException) {
            assertEquals(OAuthProblem.STORAGE, error.problem)
            assertNull(error.cause)
        }
        var cleaned = false
        try {
            withPendingOAuthGrant(
                begin = { throw OAuthProblemException(OAuthProblem.STORAGE) },
                invalidate = { cleaned = true },
            ) { fail("no owned grant") }
            fail("begin failure expected")
        } catch (error: OAuthProblemException) {
            assertEquals(OAuthProblem.STORAGE, error.problem)
            assertFalse(cleaned)
        }
    }

    @Test fun classificationUsesTypesAndNeverRawExceptionText() {
        assertEquals(OAuthProblem.TIMEOUT, classifyOAuthProblem(SocketTimeoutException("private-body")))
        assertEquals(OAuthProblem.NETWORK, classifyOAuthProblem(IOException("timeout text is not a type")))
        assertEquals(OAuthProblem.FAILED, classifyOAuthProblem(IllegalStateException("network text")))
        val cancelled = CancellationException("cancel")
        try { classifyOAuthProblem(cancelled); fail("cancellation swallowed") }
        catch (error: CancellationException) { assertSame(cancelled, error) }
        for (problem in OAuthProblem.entries) {
            val error = OAuthProblemException(problem)
            assertEquals(problem.message, error.message)
            assertNull(error.cause)
        }
    }
}
