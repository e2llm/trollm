package com.trollm.app

import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.ExecutionException
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.HttpsURLConnection
import kotlinx.coroutines.CancellationException

/** One token+JWKS attempt. A cancelled Future is not a stopped worker/socket. */
internal class MorayMachineBudget(private val totalMillis: Long) {
    private val running = AtomicBoolean(false)
    @Volatile private var fenced = false

    init { require(totalMillis in 1L..30_000L) }

    class Attempt(totalMillis: Long) {
        private val started = System.nanoTime()
        private val budget = TimeUnit.MILLISECONDS.toNanos(totalMillis)
        @Volatile private var cancelled = false
        private val lock = Any()
        private var connection: HttpsURLConnection? = null

        fun remainingNanos(): Long {
            val elapsed = System.nanoTime() - started
            return if (elapsed < 0) 0 else (budget - elapsed).coerceAtLeast(0)
        }

        fun check() {
            if (cancelled || Thread.currentThread().isInterrupted) throw CancellationException("SOURCE_MACHINE_CANCELLED")
            if (remainingNanos() == 0L) throw MorayFailure("SOURCE_MACHINE_TIMEOUT")
        }

        fun socketTimeoutMillis(): Int {
            check()
            return ((remainingNanos() + 999_999) / 1_000_000).coerceIn(1, 15_000).toInt()
        }

        fun attach(value: HttpsURLConnection) = synchronized(lock) {
            check()
            kotlin.check(connection == null)
            connection = value
        }

        fun close(value: HttpsURLConnection) {
            value.disconnect()
            synchronized(lock) { if (connection === value) connection = null }
        }

        fun cancelAndClose(): Thread? {
            cancelled = true
            val active = synchronized(lock) { connection } ?: return null
            return try {
                Thread({
                    // Disconnect itself is not assumed to be nonblocking or successful.
                    try { close(active) } catch (_: Exception) { /* retirement stays unconfirmed */ }
                }, "moray-machine-close").apply { isDaemon = true; start() }
            } catch (_: Throwable) {
                // Keep the active socket registered. The owner still reaps the
                // acquisition and must fence if local closure cannot be confirmed.
                null
            }
        }

        fun socketClosed(): Boolean = synchronized(lock) { connection == null }
    }

    fun <T> run(work: (Attempt) -> T): T {
        if (Thread.currentThread().isInterrupted) throw CancellationException("SOURCE_MACHINE_CANCELLED")
        if (fenced) throw MorayFailure("SOURCE_MACHINE_TEARDOWN_UNCONFIRMED")
        if (!running.compareAndSet(false, true)) throw MorayFailure("SOURCE_MACHINE_ACQUISITION_BUSY")
        try {
            if (fenced) throw MorayFailure("SOURCE_MACHINE_TEARDOWN_UNCONFIRMED")
            val attempt = Attempt(totalMillis)
            val task = FutureTask<T> { attempt.check(); work(attempt).also { attempt.check() } }
            val worker = Thread(task, "moray-machine-acquisition").apply { isDaemon = true }
            var result: T? = null
            var failure: Exception? = null
            var interrupted = false
            worker.start()
            try {
                result = task.get(attempt.remainingNanos(), TimeUnit.NANOSECONDS)
            } catch (_: TimeoutException) { failure = MorayFailure("SOURCE_MACHINE_TIMEOUT")
            } catch (_: InterruptedException) {
                interrupted = true
                failure = CancellationException("SOURCE_MACHINE_CANCELLED")
            } catch (error: ExecutionException) {
                failure = error.cause as? Exception ?: MorayFailure("SOURCE_MACHINE_ACQUISITION_FAILED")
            }
            val closer = if (failure != null) {
                task.cancel(true)
                attempt.cancelAndClose()
            } else null
            // One original cleanup allowance, shared by both owned threads. Never
            // re-enter acquisition if an old worker or socket cannot be retired.
            val cleanupStarted = System.nanoTime()
            fun reap(thread: Thread?) {
                if (thread == null) return
                while (thread.isAlive) {
                    val left = 1_000_000_000L - (System.nanoTime() - cleanupStarted)
                    if (left <= 0) break
                    try { thread.join(((left + 999_999) / 1_000_000).coerceAtLeast(1)) }
                    catch (_: InterruptedException) { interrupted = true }
                }
            }
            reap(worker); reap(closer)
            val retired = !worker.isAlive && closer?.isAlive != true && attempt.socketClosed()
            if (!retired) fenced = true
            if (interrupted) Thread.currentThread().interrupt()
            if (!retired) {
                val primary = when (val original = failure) {
                    is CancellationException -> "SOURCE_MACHINE_CANCELLED"
                    is MorayFailure -> original.code
                    else -> "SOURCE_MACHINE_ACQUISITION_FAILED"
                }
                if (interrupted || failure is CancellationException) throw CancellationException(primary + "_STOP_UNCONFIRMED")
                throw MorayFailure(primary + "_STOP_UNCONFIRMED")
            }
            failure?.let { throw it }
            attempt.check() // includes cleanup time; a late result cannot become usable
            @Suppress("UNCHECKED_CAST")
            return result as T
        } finally { running.set(false) }
    }
}
