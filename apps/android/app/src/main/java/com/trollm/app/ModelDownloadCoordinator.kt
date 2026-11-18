package com.trollm.app

import android.app.DownloadManager
import android.content.Context
import android.net.Uri
import android.os.Environment
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

class ModelPreparationException(message: String) : IllegalStateException(message)

/** The system owns transfers; this process only reconciles and verifies results. */
class ModelDownloadCoordinator private constructor(context: Context) {
    private val app = context.applicationContext
    private val downloads = requireNotNull(app.getSystemService(DownloadManager::class.java)) {
        "Android DownloadManager is unavailable."
    }
    private val preferences = app.getSharedPreferences("model-downloads-v1", Context.MODE_PRIVATE)
    private val lock = Mutex()
    private val root: File
        get() = requireNotNull(app.getExternalFilesDir("models")) { "Model storage is unavailable." }

    data class State(
        val path: String? = null,
        val downloaded: Long = 0,
        val total: Long = 0,
        val downloading: Boolean = false,
        val message: String? = null,
        val transferPhase: ModelTransferPhase? = null,
    )

    private data class Transfer(val status: Int, val bytes: Long, val reason: Int)

    private fun transfer(id: Long): Transfer? = downloads.query(DownloadManager.Query().setFilterById(id))?.use { cursor ->
        if (!cursor.moveToFirst()) null else Transfer(
            cursor.getInt(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS)),
            cursor.getLong(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR)).coerceAtLeast(0),
            cursor.getInt(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON)),
        )
    }

    private fun error(hash: String, message: String) {
        preferences.edit().putString("error.$hash", message).commit()
    }

    private fun removeOrphanedPartials() {
        val tracked = preferences.all.filterKeys { it.startsWith("download.") }.values.filterIsInstance<Long>().toSet()
        val orphaned = mutableListOf<Long>()
        var unidentified = false
        // Query exposes only downloads owned by this application. Bound work even
        // if a future app feature leaves a large DownloadManager history.
        val rowsCursor = downloads.query(DownloadManager.Query())
            ?: throw ModelPreparationException("Could not inspect existing Android downloads.")
        rowsCursor.use { cursor ->
            var rows = 0
            while (cursor.moveToNext()) {
                ready(++rows <= 512, "Too many download records to reconcile safely.")
                val id = cursor.getLong(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_ID))
                if (id in tracked) continue
                val localUri = cursor.getString(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_LOCAL_URI))
                if (ModelFiles.isUntrackedPartial(id, tracked, localUri, root)) {
                    orphaned.add(id)
                } else if (localUri == null) {
                    val status = cursor.getInt(cursor.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS))
                    if (status == DownloadManager.STATUS_PENDING || status == DownloadManager.STATUS_RUNNING || status == DownloadManager.STATUS_PAUSED) unidentified = true
                }
            }
        }
        orphaned.forEach { downloads.remove(it) }
        ready(!unidentified, "A pending download has no identifiable destination yet. Try again after Android updates its status.")
    }

    private fun forgetTransfer(hash: String) {
        val id = preferences.getLong("download.$hash", -1)
        if (id >= 0) downloads.remove(id)
        preferences.edit().remove("download.$hash").remove("size.$hash").remove("metered.$hash").commit()
    }

    private fun receiptMatches(model: ModelRequirement): File? {
        val path = preferences.getString("verified.${model.sha256}.path", null) ?: return null
        val file = File(path)
        val allowed = file.canonicalFile.parentFile == root.canonicalFile ||
            file.canonicalFile.parentFile == File(app.filesDir, "trollm/models").canonicalFile
        if (!allowed || !file.isFile || file.length() != model.sizeBytes ||
            preferences.getLong("verified.${model.sha256}.size", -1) != file.length() ||
            preferences.getLong("verified.${model.sha256}.mtime", -1) != file.lastModified()
        ) return null
        return file
    }

    private fun receipt(model: ModelRequirement, file: File) = receipt(model.sha256, file)

    private fun receipt(hash: String, file: File) {
        preferences.edit().putString("verified.$hash.path", file.canonicalPath)
            .putLong("verified.$hash.size", file.length())
            .putLong("verified.$hash.mtime", file.lastModified())
            .remove("error.$hash").commit()
    }

    // Reconcile transfers started on a different setup screen as well. The ID,
    // expected hash and byte count are app-owned; broadcasts supply no metadata.
    private suspend fun reconcileOtherTransfers(setup: FlowSetup) {
        val current = setup.models.values.map { it.sha256 }.toSet()
        for ((key, value) in preferences.all.filterKeys { it.startsWith("download.") }) {
            val hash = key.removePrefix("download.")
            if (hash in current) continue
            val id = value as? Long ?: continue
            val item = transfer(id)
            if (item != null && item.status != DownloadManager.STATUS_FAILED && item.status != DownloadManager.STATUS_SUCCESSFUL) continue
            try {
                check(item?.status == DownloadManager.STATUS_SUCCESSFUL)
                val expectedSize = preferences.getLong("size.$hash", -1)
                val final = File(root, "$hash.gguf")
                val candidate = if (final.exists()) final else File(root, "$hash.download")
                val coroutine = currentCoroutineContext()
                ModelFiles.verify(candidate, expectedSize, hash) { coroutine.ensureActive() }
                if (candidate != final) ModelFiles.publish(candidate, final)
                receipt(hash, final)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                error(hash, "Previous model download failed verification or transfer. Retry from its setup screen.")
            }
            downloads.remove(id)
            preferences.edit().remove(key).remove("size.$hash").remove("metered.$hash").commit()
        }
    }

    private suspend fun verify(model: ModelRequirement, file: File) {
        val coroutine = currentCoroutineContext()
        ModelFiles.verify(file, model.sizeBytes, model.sha256) { coroutine.ensureActive() }
    }

    suspend fun inspect(setup: FlowSetup, onVerifying: suspend (String) -> Unit = {}): Map<String, State> = withContext(Dispatchers.IO) {
        lock.withLock {
            removeOrphanedPartials()
            reconcileOtherTransfers(setup)
            val states = linkedMapOf<String, State>()
            for ((slot, model) in setup.models) {
                try {
                    receiptMatches(model)?.let {
                        forgetTransfer(model.sha256)
                        states[slot] = State(path = it.canonicalPath, total = model.sizeBytes)
                    }
                    if (states.containsKey(slot)) continue
                    val final = File(root, "${model.sha256}.gguf")
                    val legacy = File(app.filesDir, "trollm/models/$slot.gguf")
                    val id = preferences.getLong("download.${model.sha256}", -1)
                    if (final.exists()) {
                        val rejected = preferences.getLong("rejected.${model.sha256}.mtime", -1) == final.lastModified() &&
                            preferences.getLong("rejected.${model.sha256}.size", -1) == final.length()
                        if (!rejected) {
                            onVerifying(slot)
                            try {
                                verify(model, final)
                                receipt(model, final)
                                states[slot] = State(path = final.canonicalPath, total = model.sizeBytes)
                                forgetTransfer(model.sha256)
                                continue
                            } catch (cancelled: CancellationException) {
                                throw cancelled
                            } catch (_: Exception) {
                                preferences.edit().putLong("rejected.${model.sha256}.mtime", final.lastModified())
                                    .putLong("rejected.${model.sha256}.size", final.length()).commit()
                            }
                        }
                        forgetTransfer(model.sha256)
                        states[slot] = State(total = model.sizeBytes, message = "The downloaded model failed verification. Retry replaces the corrupt download.")
                        continue
                    }
                    // Existing legacy weights are only adopted on an exact match.
                    if (legacy.isFile && legacy.length() == model.sizeBytes &&
                        preferences.getLong("legacy-rejected.${model.sha256}.$slot", -1) != legacy.lastModified()
                    ) {
                        onVerifying(slot)
                        try {
                            verify(model, legacy)
                            receipt(model, legacy)
                            forgetTransfer(model.sha256)
                            states[slot] = State(path = legacy.canonicalPath, total = model.sizeBytes)
                            continue
                        } catch (cancelled: CancellationException) {
                            throw cancelled
                        } catch (_: Exception) {
                            preferences.edit().putLong("legacy-rejected.${model.sha256}.$slot", legacy.lastModified()).commit()
                        }
                    }
                    val transfer = if (id >= 0) transfer(id) else null
                    if (transfer?.status == DownloadManager.STATUS_SUCCESSFUL) {
                        onVerifying(slot)
                        val partial = File(root, "${model.sha256}.download")
                        try {
                            verify(model, partial)
                            ModelFiles.publish(partial, final)
                            receipt(model, final)
                            forgetTransfer(model.sha256)
                            states[slot] = State(path = final.canonicalPath, total = model.sizeBytes)
                        } catch (cancelled: CancellationException) {
                            throw cancelled
                        } catch (_: Exception) {
                            forgetTransfer(model.sha256)
                            error(model.sha256, "Downloaded model failed size/checksum verification or installation. Retry the download.")
                            states[slot] = State(total = model.sizeBytes, message = preferences.getString("error.${model.sha256}", null))
                        }
                    } else if (transfer != null && transfer.status != DownloadManager.STATUS_FAILED) {
                        val policy = if (preferences.contains("metered.${model.sha256}"))
                            ModelDownloadPolicy(preferences.getBoolean("metered.${model.sha256}", false)) else null
                        states[slot] = State(downloaded = transfer.bytes, total = model.sizeBytes, downloading = true,
                            transferPhase = when (transfer.status) {
                                DownloadManager.STATUS_PENDING -> ModelTransferPhase.QUEUED
                                DownloadManager.STATUS_PAUSED -> when (transfer.reason) {
                                    DownloadManager.PAUSED_WAITING_FOR_NETWORK, DownloadManager.PAUSED_QUEUED_FOR_WIFI ->
                                        ModelTransferPhase.WAITING_FOR_NETWORK
                                    else -> ModelTransferPhase.PAUSED
                                }
                                else -> ModelTransferPhase.TRANSFERRING
                            },
                            message = when {
                                root.usableSpace < MODEL_SPACE_HEADROOM || (model.sizeBytes - transfer.bytes).coerceAtLeast(0) > root.usableSpace - MODEL_SPACE_HEADROOM -> "Low free space for the remaining download."
                                policy == null -> "Existing download has no recorded network policy. Cancel and retry to apply this screen's network choice."
                                transfer.status == DownloadManager.STATUS_PENDING -> policy.description
                                transfer.status == DownloadManager.STATUS_PAUSED -> when (transfer.reason) {
                                    DownloadManager.PAUSED_WAITING_FOR_NETWORK -> policy.waitingMessage()
                                    DownloadManager.PAUSED_QUEUED_FOR_WIFI -> "Waiting for Wi-Fi or another unmetered network."
                                    else -> "Download paused; Android will retry."
                                }
                                else -> "Downloading. ${policy.description}"
                            })
                    } else {
                        if (id >= 0) {
                            error(model.sha256, if (transfer == null) "Download is no longer available. Retry." else "Download failed (Android code ${transfer.reason}). Retry.")
                            forgetTransfer(model.sha256)
                        }
                        states[slot] = State(total = model.sizeBytes, message = preferences.getString("error.${model.sha256}", null))
                    }
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    error(model.sha256, "Could not verify or access this model. Check available storage and retry.")
                    states[slot] = State(total = model.sizeBytes, message = preferences.getString("error.${model.sha256}", null))
                }
            }
            states
        }
    }

    /** Admit one transfer globally, including one started for a different flow. */
    suspend fun download(model: ModelRequirement, policy: ModelDownloadPolicy = ModelDownloadPolicy()) = withContext(Dispatchers.IO) {
        try {
            lock.withLock {
                model.validate()
                removeOrphanedPartials()
                if (receiptMatches(model) != null) return@withLock
                val active = preferences.all.filterKeys { it.startsWith("download.") }
                ready(active.isEmpty(), "Finish or cancel the current model download first.")
                val directory = root.also { ready(it.exists() || it.mkdirs(), "Model storage is unavailable.") }
                ready(Environment.getExternalStorageState(directory) == Environment.MEDIA_MOUNTED, "Model storage is unavailable.")
                val coroutine = currentCoroutineContext()
                val validFinal = ModelFiles.repairFinal(directory, model.sizeBytes, model.sha256,
                    preferences.getLong("rejected.${model.sha256}.size", -1),
                    preferences.getLong("rejected.${model.sha256}.mtime", -1)) { coroutine.ensureActive() }
                if (validFinal != null) {
                    receipt(model, validFinal)
                    return@withLock
                }
                preferences.edit().remove("rejected.${model.sha256}.size").remove("rejected.${model.sha256}.mtime").commit()
                val partial = File(directory, "${model.sha256}.download")
                // Only our incomplete target is disposable, never legacy/final weights.
                ready(!partial.exists() || partial.delete(), "Could not remove an incomplete download.")
                val available = directory.usableSpace
                ready(available >= MODEL_SPACE_HEADROOM && model.sizeBytes <= available - MODEL_SPACE_HEADROOM, "Not enough space: the model size plus 256 MiB free is required.")
                val request = DownloadManager.Request(Uri.parse(model.url))
                    .setAllowedOverMetered(policy.allowMetered)
                    .setAllowedOverRoaming(policy.allowRoaming)
                    .setTitle("trollm model: ${model.label}")
                    .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                    .setDestinationInExternalFilesDir(app, "models", partial.name)
                val id = downloads.enqueue(request)
                if (!preferences.edit().putLong("download.${model.sha256}", id).putLong("size.${model.sha256}", model.sizeBytes)
                        .putBoolean("metered.${model.sha256}", policy.allowMetered).remove("error.${model.sha256}").commit()) {
                    downloads.remove(id)
                    throw ModelPreparationException("Could not persist the model download.")
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: ModelPreparationException) {
            error(model.sha256, failure.message ?: "Could not start the download.")
            throw failure
        } catch (failure: Exception) {
            error(model.sha256, "Could not start the model download. Check storage and retry.")
            throw failure
        }
    }

    suspend fun cancel(model: ModelRequirement) = withContext(Dispatchers.IO) {
        lock.withLock {
            val id = preferences.getLong("download.${model.sha256}", -1)
            if (id >= 0) downloads.remove(id)
            File(root, "${model.sha256}.download").delete()
            preferences.edit().remove("download.${model.sha256}").remove("size.${model.sha256}")
                .remove("metered.${model.sha256}").putString("error.${model.sha256}", "Download cancelled. You can retry.").commit()
        }
    }

    private fun ready(condition: Boolean, message: String) {
        if (!condition) throw ModelPreparationException(message)
    }

    companion object {
        @Volatile private var instance: ModelDownloadCoordinator? = null
        fun get(context: Context): ModelDownloadCoordinator = instance ?: synchronized(this) {
            instance ?: ModelDownloadCoordinator(context).also { instance = it }
        }
    }
}
