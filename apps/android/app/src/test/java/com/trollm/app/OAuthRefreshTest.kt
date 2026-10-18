package com.trollm.app

import java.io.IOException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.*
import org.junit.Test

class OAuthRefreshTest {
    private class Memory {
        private val values = mutableMapOf<String, StoredConnection>()
        var failReads = false
        var failWrites = false
        val grants = OAuthGrants(
            { if (failReads) throw IOException("private-read-state")
                values[it] ?: StoredConnection(null, null, null) },
            { name, value ->
                if (failWrites) throw IOException("private-write-token")
                values[name] = value
            }, Any(),
        )
        fun login(server: String = "browser"): OAuthGrant {
            val grant = grants.begin(server, "https://example.test/mcp", "pending", "request")
            check(grants.update(server, grant.id, grant.endpoint, "initial-state"))
            return grants.current(server)!!
        }
    }

    @Test fun synchronousCallbackIsPublishedBeforeSchedulingAndRotationPrecedesTokenExposure() {
        val memory = Memory()
        val grant = memory.login()
        val seen = mutableListOf<String>()
        val refresh = OAuthRefresh(memory.grants, { it() }, { current, callback ->
            seen += current.state
            callback(OAuthRefreshResult("private-access", "rotated-${seen.size}"))
        })
        assertEquals("private-access", refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals("rotated-1", memory.grants.current("browser")?.state)
        assertEquals("private-access", refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals(listOf("initial-state", "rotated-1"), seen)
        assertEquals("rotated-2", memory.grants.current("browser")?.state)
        assertNull(refresh.problem("browser", grant.endpoint, grant.id))
    }

    @Test fun timedOutWaitersStayCoalescedUntilCallbackAndLateSuccessClearsProblem() {
        val memory = Memory()
        val grant = memory.login()
        val calls = AtomicInteger()
        lateinit var callback: (OAuthRefreshResult) -> Unit
        // Zero wait is deterministic: the fake SDK retains its callback.
        val refresh = OAuthRefresh(memory.grants, { it() }, { _, result ->
            calls.incrementAndGet()
            callback = result
        }, timeoutMillis = 0)
        repeat(2) {
            assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        }
        assertEquals(1, calls.get())
        assertEquals(OAuthProblem.TIMEOUT, refresh.problem("browser", grant.endpoint, grant.id))
        callback(OAuthRefreshResult("private-token", "rotated-after-timeout"))
        assertEquals("rotated-after-timeout", memory.grants.current("browser")?.state)
        assertNull(refresh.problem("browser", grant.endpoint, grant.id))
        // A duplicate callback must not overwrite a later rotation.
        callback(OAuthRefreshResult("private-token", "stale-duplicate"))
        assertEquals("rotated-after-timeout", memory.grants.current("browser")?.state)
    }

    @Test fun persistenceFailureNeverExposesTokenAndSdkErrorsStillPersistRotatedState() {
        val memory = Memory()
        val grant = memory.login()
        var problem: OAuthProblem? = null
        val refresh = OAuthRefresh(memory.grants, { it() }, { _, callback ->
            callback(OAuthRefreshResult("private-access", "rotated-state", problem))
        })
        memory.failWrites = true
        assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals(OAuthProblem.STORAGE, refresh.problem("browser", grant.endpoint, grant.id))
        assertEquals("initial-state", memory.grants.current("browser")?.state)
        memory.failWrites = false
        problem = OAuthProblem.NETWORK
        assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals("rotated-state", memory.grants.current("browser")?.state)
        assertEquals(OAuthProblem.NETWORK, refresh.problem("browser", grant.endpoint, grant.id))
        problem = null
        assertEquals("private-access", refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertNull(refresh.problem("browser", grant.endpoint, grant.id))
        memory.failReads = true
        assertEquals(OAuthProblem.STORAGE, refresh.problem("browser", grant.endpoint, grant.id))
    }

    @Test fun lateCallbacksCannotReplaceNewGrantLogoutOrEndpointSelection() {
        for (replacement in listOf(false, true)) {
            val memory = Memory()
            val grant = memory.login()
            lateinit var callback: (OAuthRefreshResult) -> Unit
            val refresh = OAuthRefresh(memory.grants, { it() }, { _, result -> callback = result }, 0)
            assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
            val next = if (replacement) memory.login() else null
            if (!replacement) memory.grants.logout("browser")
            callback(OAuthRefreshResult("private-late-token", "private-late-state"))
            assertEquals(next?.id, memory.grants.current("browser")?.id)
            if (next != null) assertEquals("initial-state", memory.grants.current("browser")?.state)
            assertNull(refresh.problem("browser", grant.endpoint, grant.id))
            assertNull(memory.grants.manualToken("browser", grant.endpoint))
            assertNull(refresh.accessTokenBlocking("browser", "https://example.test/other", grant.id))
        }
    }

    @Test fun logoutDuringFinalIdentityCheckRejectsAlreadyPersistedToken() {
        val memory = Memory()
        val grant = memory.login()
        val schedules = AtomicInteger()
        val refresh = OAuthRefresh(memory.grants, { work ->
            if (schedules.incrementAndGet() == 3) memory.grants.logout("browser")
            work()
        }, { _, callback -> callback(OAuthRefreshResult("private-token", "rotated")) })
        assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertNull(memory.grants.current("browser"))
    }

    @Test fun interruptedWaitPreservesInterruptAndInFlightSdkRequest() {
        val memory = Memory()
        val grant = memory.login()
        val entered = CountDownLatch(1)
        val interrupted = AtomicBoolean(false)
        val returned = CompletableFuture<String?>()
        val calls = AtomicInteger()
        lateinit var callback: (OAuthRefreshResult) -> Unit
        val refresh = OAuthRefresh(memory.grants, { it() }, { _, result ->
            callback = result
            calls.incrementAndGet()
            entered.countDown()
        })
        val worker = Thread {
            returned.complete(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
            interrupted.set(Thread.currentThread().isInterrupted)
        }
        worker.start()
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        worker.interrupt()
        worker.join(5_000)
        assertFalse(worker.isAlive)
        assertNull(returned.get(1, TimeUnit.SECONDS))
        assertTrue(interrupted.get())
        assertEquals(OAuthProblem.CANCELLED, refresh.problem("browser", grant.endpoint, grant.id))
        assertEquals(1, calls.get())
        callback(OAuthRefreshResult("private-token", "rotated-after-interrupt"))
        assertNull(refresh.problem("browser", grant.endpoint, grant.id))
        assertEquals("rotated-after-interrupt", memory.grants.current("browser")?.state)
    }

    @Test fun diagnosticsAreBoundedExactGrantScopedAndRedacted() {
        val memory = Memory()
        val refresh = OAuthRefresh(memory.grants, { it() }, { _, callback ->
            callback(OAuthRefreshResult(null, "rotated-state", OAuthProblem.NETWORK))
        })
        val grants = (0..32).map { index -> memory.login("s$index") }
        for (grant in grants) assertNull(refresh.accessTokenBlocking(grant.server, grant.endpoint, grant.id))
        assertNull(refresh.problem(grants.first().server, grants.first().endpoint, grants.first().id))
        val last = grants.last()
        assertEquals(OAuthProblem.NETWORK, refresh.problem(last.server, last.endpoint, last.id))
        assertNull(refresh.problem(last.server, "https://example.test/other", last.id))
        refresh.clearServer(last.server)
        assertNull(refresh.problem(last.server, last.endpoint, last.id))
        assertEquals("OAuthRefreshResult(redacted)",
            OAuthRefreshResult("private-token", "private-state", OAuthProblem.FAILED).toString())
    }

    @Test fun lateWaiterCannotReplaceNewFlightOrEvictedDiagnostics() {
        for (mode in listOf("active", "success", "evicted")) {
            val memory = Memory()
            val grant = memory.login()
            val schedules = AtomicInteger()
            val browserRequests = AtomicInteger()
            val oldCheckScheduled = CountDownLatch(1)
            val rejectOldCheck = CountDownLatch(1)
            val secondStarted = CountDownLatch(1)
            val serialStore = Any()
            lateinit var secondCallback: (OAuthRefreshResult) -> Unit
            val refresh = OAuthRefresh(memory.grants, { work ->
                if (schedules.incrementAndGet() == 3) {
                    // Defer the old waiter's final-check scheduling failure.
                    // Other accepted store tasks still run serially, and no
                    // credential operation from this rejected task executes.
                    oldCheckScheduled.countDown()
                    rejectOldCheck.await()
                    throw java.util.concurrent.RejectedExecutionException("private-scheduler-detail")
                }
                synchronized(serialStore) { work() }
            }, { current, callback ->
                if (current.server != "browser" || browserRequests.incrementAndGet() == 1) {
                    callback(OAuthRefreshResult("private-token", "rotated"))
                } else {
                    secondCallback = callback
                    secondStarted.countDown()
                }
            })
            fun request(): Pair<Thread, CompletableFuture<String?>> {
                val result = CompletableFuture<String?>()
                val worker = Thread {
                    try { result.complete(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id)) }
                    catch (error: Throwable) { result.completeExceptionally(error) }
                }
                worker.start()
                return worker to result
            }
            val (oldWorker, oldResult) = request()
            var newer: Pair<Thread, CompletableFuture<String?>>? = null
            try {
                assertTrue(oldCheckScheduled.await(5, TimeUnit.SECONDS))
                if (mode == "evicted") {
                    // Successful/null records count toward the 32 latest markers.
                    repeat(32) { index ->
                        val other = memory.login("other$index")
                        assertEquals("private-token", refresh.accessTokenBlocking(other.server, other.endpoint, other.id))
                    }
                } else {
                    newer = request()
                    assertTrue(secondStarted.await(5, TimeUnit.SECONDS))
                    if (mode == "success") {
                        secondCallback(OAuthRefreshResult("private-new-token", "newer-rotation"))
                        assertEquals("private-new-token", newer.second.get(5, TimeUnit.SECONDS))
                    }
                }
                rejectOldCheck.countDown()
                assertNull(oldResult.get(5, TimeUnit.SECONDS))
                oldWorker.join(5_000)
                assertFalse(oldWorker.isAlive)
                // The old STORAGE failure must not replace a newer null entry,
                // either while its SDK request is active or after its success.
                // An evicted marker must never be resurrected by the old waiter.
                assertNull("late error replaced $mode diagnostics", refresh.problem("browser", grant.endpoint, grant.id))
            } finally {
                rejectOldCheck.countDown()
                if (mode == "active" && secondStarted.count == 0L) {
                    secondCallback(OAuthRefreshResult("private-new-token", "newer-rotation"))
                }
                oldWorker.join(5_000)
                newer?.first?.join(5_000)
            }
        }
    }

    @Test fun adapterExceptionsBecomeSafeCategoriesWithoutRetainingRawCauses() {
        val memory = Memory()
        val grant = memory.login()
        val refresh = OAuthRefresh(memory.grants, { it() }, { _, _ ->
            throw java.net.SocketTimeoutException("private-response-body")
        })
        assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals(OAuthProblem.TIMEOUT, refresh.problem("browser", grant.endpoint, grant.id))
        assertEquals("initial-state", memory.grants.current("browser")?.state)
    }

    @Test fun invalidTokenOrMissingStateCannotBypassPersistenceRequirement() {
        val memory = Memory()
        val grant = memory.login()
        var result = OAuthRefreshResult("bad\nheader", "rotated")
        val refresh = OAuthRefresh(memory.grants, { it() }, { _, callback -> callback(result) })
        assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals(OAuthProblem.RECONNECT, refresh.problem("browser", grant.endpoint, grant.id))
        result = OAuthRefreshResult("private-valid-token", null)
        assertNull(refresh.accessTokenBlocking("browser", grant.endpoint, grant.id))
        assertEquals(OAuthProblem.FAILED, refresh.problem("browser", grant.endpoint, grant.id))
    }
}
