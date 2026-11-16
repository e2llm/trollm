package com.trollm.app

import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.system.Os
import android.system.OsConstants
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import uniffi.trollm_uniffi.TrollmException

/** Private debug installation channel. Intents carry only a request-file hash.
 * App UID filesystem custody, the exact APK hash and the live handle establish
 * the association; a caller-supplied JSON verdict does not authenticate delivery.
 */
internal object SelectedNativeLauncher {
    internal fun retainClosedStartFailure(result: JSONObject, error: Exception) {
        (error as? TrollmException.SelectedStart)?.let { diagnostic ->
            if (Regex("SELECTED_[A-Z0-9_]{1,80}").matches(diagnostic.code))
                result.put("nativeCause", diagnostic.code)
            diagnostic.nativeRunId?.takeIf {
                Regex("[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}").matches(it)
            }?.let { result.put("nativeRunId", it) }
        }
    }
    const val ACTION = "com.trollm.app.PRIVATE_SELECTED_REQUEST"
    const val EXTRA = "requestSha256"
    internal fun requestExecutor() = ThreadPoolExecutor(
        1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue<Runnable>(8),
        ThreadPoolExecutor.AbortPolicy(),
    )
    private val executor = requestExecutor()
    private val handles = mutableMapOf<String, SelectedNativeRun>()
    private val hashPattern = Regex("[a-f0-9]{64}")

    fun accept(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        val key = intent.getStringExtra(EXTRA)
        intent.removeExtra(EXTRA)
        if (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE == 0 ||
            key == null || !hashPattern.matches(key)) return
        val app = context.applicationContext
        try {
            executor.execute { process(app, key) }
        } catch (_: RejectedExecutionException) {
            // No claim is created for a request that never enters the queue.
        }
    }

    private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256")
        .digest(bytes).joinToString("") { "%02x".format(it.toInt() and 255) }

    private fun privateBytes(file: File, limit: Int): ByteArray {
        val before = Os.lstat(file.path)
        require(OsConstants.S_ISREG(before.st_mode) && before.st_uid == android.os.Process.myUid())
        require(before.st_mode and 0x3f == 0 && before.st_size in 1..limit.toLong())
        return file.inputStream().use { input ->
            val opened = Os.fstat(input.fd)
            require(opened.st_ino == before.st_ino && opened.st_dev == before.st_dev)
            val output = java.io.ByteArrayOutputStream()
            val buffer = ByteArray(8192)
            while (output.size() <= limit) {
                val n = input.read(buffer, 0, minOf(buffer.size, limit + 1 - output.size()))
                if (n < 0) break
                output.write(buffer, 0, n)
            }
            val bytes = output.toByteArray()
            val after = Os.lstat(file.path)
            require(bytes.size.toLong() == before.st_size && bytes.size <= limit &&
                after.st_ino == before.st_ino && after.st_dev == before.st_dev &&
                after.st_size == before.st_size && after.st_mtime == before.st_mtime)
            bytes
        }
    }

    private fun exact(value: JSONObject, keys: Set<String>) {
        require(value.keys().asSequence().toSet() == keys)
    }

    internal fun validateRequest(request: JSONObject, sourceRevision: String): String {
        val operation = request.getString("operation")
        require(operation in setOf("start", "binding", "cancel"))
        val common = setOf("schema", "operation", "apkSha256", "invocationKey", "installationKey",
            "requestId", "sourceRevision")
        val fields = if (operation == "start") common + setOf("flowSha256", "installationSha256",
            "input", "configurationJson", "associationJson") else common + "nativeRunId"
        exact(request, fields)
        require(fields.all { request.get(it) is String })
        require(request.getString("schema") == "trollm.private-selected-launcher-request.v1")
        require(request.getString("sourceRevision") == sourceRevision)
        require(Regex("[a-f0-9]{32}").matches(request.getString("requestId")))
        require(hashPattern.matches(request.getString("apkSha256")))
        require(hashPattern.matches(request.getString("installationKey")))
        require(request.getString("invocationKey").isNotEmpty() && request.getString("invocationKey").length <= 256)
        if (operation == "start") {
            require(hashPattern.matches(request.getString("flowSha256")))
            require(hashPattern.matches(request.getString("installationSha256")))
        } else require(RunIdentity.valid(request.getString("nativeRunId")))
        return operation
    }

    private fun writeNew(file: File, bytes: ByteArray) {
        // CREATE_NEW is the reservation. An interrupted start is never retried.
        val fd = Os.open(file.path, OsConstants.O_WRONLY or OsConstants.O_CREAT or
            OsConstants.O_EXCL or OsConstants.O_NOFOLLOW, 0x180)
        FileOutputStream(fd).use { it.write(bytes); it.fd.sync() }
    }

    private fun process(context: Context, key: String) {
        val directory = File(context.filesDir, "trollm/selected-launcher")
        val parent = runCatching { Os.lstat(directory.path) }.getOrNull() ?: return
        if (!OsConstants.S_ISDIR(parent.st_mode) || parent.st_uid != android.os.Process.myUid() ||
            parent.st_mode and 0x3f != 0 ||
            directory.canonicalFile != File(context.filesDir.canonicalFile, "trollm/selected-launcher")) return
        // An arbitrary intent hash must not create files. Only a private,
        // hash-matched, structurally valid request can reserve a claim.
        val request = try {
            val bytes = privateBytes(File(directory, "$key.request.json"), 262144)
            require(sha(bytes) == key)
            JSONObject(bytes.toString(Charsets.UTF_8)).also {
                validateRequest(it, BuildConfig.SOURCE_REVISION)
            }
        } catch (_: Exception) {
            return
        }
        val claim = File(directory, "$key.claim")
        // Duplicate intents cannot start again, including after a process crash.
        try { writeNew(claim, byteArrayOf(1)) } catch (_: Exception) { return }
        var run: SelectedNativeRun? = null
        var stage = "installed-apk"
        val result = try {
            val operation = request.getString("operation")
            val expectedApk = request.getString("apkSha256")
            require(hashPattern.matches(expectedApk))
            // Refuse split APKs in this bounded profile. Hash actual installed bytes.
            require(context.applicationInfo.splitSourceDirs.isNullOrEmpty())
            val digest = MessageDigest.getInstance("SHA-256")
            val apk = File(context.applicationInfo.sourceDir)
            require(apk.length() in 1..134217728L)
            apk.inputStream().use { input ->
                val buffer = ByteArray(65536)
                var total = 0L
                while (true) {
                    val n = input.read(buffer)
                    if (n < 0) break
                    total += n
                    require(total <= 134217728L)
                    digest.update(buffer, 0, n)
                }
            }
            require(digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) } == expectedApk)
            val invocation = request.getString("invocationKey")
            val installation = request.getString("installationKey")
            require(hashPattern.matches(installation))
            if (operation == "start") {
                require(!handles.containsKey(invocation))
                stage = "reserve-invocation"
                // Reserve the invocation separately from the request hash. A changed
                // start request cannot create a second worker for this invocation.
                writeNew(File(directory, "invocation-${sha(invocation.toByteArray())}.claim"), key.toByteArray())
                stage = "native-start"
                run = SelectedNativeRun.start(context, request.getString("flowSha256"),
                    request.getString("installationSha256"), request.getString("input"),
                    request.getString("configurationJson"), request.getString("associationJson"),
                    installation, invocation) { stage = it }
                val started = checkNotNull(run)
                handles[invocation] = started
                stage = "retain-started-id"
                // Retain genuine ID before any binding/service/report operation.
                writeNew(File(directory, "$key.started.json"), JSONObject()
                    .put("nativeRunId", started.runId).put("invocationKey", invocation).toString().toByteArray())
                RunHistory(context).record(started.runId)
                RunService.start(context, started.runId)
            } else {
                stage = "same-process-handle"
                require(operation == "binding" || operation == "cancel")
                run = checkNotNull(handles[invocation]) { "same-process selected handle unavailable" }
                require(checkNotNull(run).runId == request.getString("nativeRunId"))
            }
            val active = checkNotNull(run)
            stage = "native-binding"
            val binding = active.binding()
            val original = JSONObject(binding.getString("privateBindingJson"))
            require(original.getString("privateInstallationSha256") == installation)
            if (operation == "cancel") active.cancel()
            JSONObject().put("schema", "trollm.private-selected-launcher-result.v1")
                .put("requestSha256", key).put("apkSha256", expectedApk)
                .put("sourceRevision", BuildConfig.SOURCE_REVISION)
                .put("invocationKey", invocation).put("binding", active.binding())
        } catch (error: Exception) {
            // No raw exception: source input, credentials and model text stay private.
            JSONObject().put("schema", "trollm.private-selected-launcher-result.v1")
                .put("requestSha256", key).put("error", "SELECTED_LAUNCHER_REFUSED")
                .put("stage", stage)
                .also { result ->
                    run?.let { result.put("nativeRunId", it.runId) }
                    retainClosedStartFailure(result, error)
                }
        }
        runCatching {
            writeNew(File(directory, "$key.result.json"), result.toString().toByteArray())
            writeNew(File(directory, "$key.done"), byteArrayOf(1))
        }
    }
}
