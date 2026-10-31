package com.trollm.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.TrollmRuntime

/**
 * Foreground service holding a partial wake lock while a run is active
 * (§10). The run executes on Rust-side threads; this service's only jobs
 * are to support execution across UI backgrounding, hold the wake
 * lock, and reflect progress in the notification. It observes the run the
 * same way the UI does: `observe_run` over the UniFFI surface.
 */
class RunService : Service() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    // Mutated only on the service main thread, including observer completion.
    private val activeRuns = ActiveRuns()
    private val observationVersions = mutableMapOf<String, Long>()
    private var wakeLock: PowerManager.WakeLock? = null
    private val wakeLease = RunWakeLease()
    private var destroyed = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val runId = intent?.getStringExtra(EXTRA_RUN_ID)?.takeIf(RunIdentity::valid)
        if (runId == null) {
            if (activeRuns.isEmpty()) stopSelf()
            return START_NOT_STICKY
        }
        observationVersions[runId] = (observationVersions[runId] ?: 0L) + 1L
        if (!activeRuns.attach(runId)) return START_NOT_STICKY
        startForeground(NOTIFICATION_ID, notification(getString(R.string.notification_run_active), runId))
        acquireWakeLock()

        val runtime = RuntimeHolder.get(this)
        observe(runId, runtime)
        // A cold process needs an explicit UI resume, not an observer of stale status.
        return START_NOT_STICKY
    }

    private fun observe(runId: String, runtime: TrollmRuntime) {
        val version = observationVersions[runId]
        scope.launch {
            var cursor = 0uL
            var previousText = getString(R.string.notification_run_active)
            while (isActive) {
                // A repeated start can change the generation without replacing
                // this observer. Snapshot it for each fresh read, not just once.
                val readVersion = withContext(Dispatchers.Main) {
                    observationVersions[runId].takeUnless { destroyed }
                } ?: break
                val presentation = runCatching {
                    val observation = runtime.observeRun(runId, cursor)
                    val status = observation.status
                    cursor = status.lastSeq
                    presentRun(status.phase, status.workerActive, status.activity)
                }.getOrNull() // Read errors keep ownership, but cannot renew the timed lease.
                if (presentation != null) {
                    if (!presentation.observeInBackground) break
                    withContext(Dispatchers.Main) {
                        if (!destroyed && observationVersions[runId] == readVersion &&
                            wakeLease.shouldRenew(SystemClock.elapsedRealtime(), presentation.observeInBackground)) {
                            wakeLock?.let {
                                it.acquire(RunWakeLease.TIMEOUT_MS)
                                wakeLease.acquired(SystemClock.elapsedRealtime())
                            }
                        }
                    }
                    if (presentation.text != previousText &&
                        ContextCompat.checkSelfPermission(this@RunService, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
                        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
                        manager.notify(NOTIFICATION_ID, notification(presentation.text, runId))
                        previousText = presentation.text
                    }
                }
                delay(1000)
            }
            withContext(Dispatchers.Main) {
                if (destroyed) return@withContext
                // Resume may restart observation while a paused observer is
                // detaching. Honor that newer request before releasing ownership.
                if (observationVersions[runId] != version) {
                    observe(runId, runtime)
                    return@withContext
                }
                observationVersions.remove(runId)
                if (activeRuns.complete(runId)) {
                    releaseWakeLock()
                    stopForeground(STOP_FOREGROUND_REMOVE)
                    stopSelf()
                } else {
                    val owner = activeRuns.first()!!
                    startForeground(NOTIFICATION_ID, notification(getString(R.string.notification_run_active), owner))
                }
            }
        }
    }

    override fun onDestroy() {
        destroyed = true
        scope.cancel()
        releaseWakeLock()
        super.onDestroy()
    }

    private fun acquireWakeLock() {
        // Repeated starts are not fresh evidence that a worker is still active.
        if (wakeLock != null) return
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "trollm:run").apply {
            // Non-reference-counted reacquisition replaces the timed release;
            // one explicit release still clears the whole lease.
            setReferenceCounted(false)
            acquire(RunWakeLease.TIMEOUT_MS)
        }
        wakeLease.acquired(SystemClock.elapsedRealtime())
    }

    private fun releaseWakeLock() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        wakeLease.reset()
    }

    private fun notification(text: String, runId: String): Notification {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, getString(R.string.channel_runs),
                NotificationManager.IMPORTANCE_LOW)
        )
        val openIntent = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java)
                .putExtra(EXTRA_RUN_ID, runId)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(text)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentIntent(openIntent)
            .setOngoing(true)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "runs"
        private const val NOTIFICATION_ID = 1
        const val EXTRA_RUN_ID = "run_id"

        fun start(context: Context, runId: String) {
            val intent = Intent(context, RunService::class.java)
                .putExtra(EXTRA_RUN_ID, runId)
            context.startForegroundService(intent)
        }
    }
}
