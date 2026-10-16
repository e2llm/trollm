package com.trollm.app

import java.io.File
import java.net.URI
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

/** File operations have no Android dependency and never buffer model weights. */
object ModelFiles {
    /** Explicit retry may repair only this store's exact hash-named final. */
    fun repairFinal(
        directory: File,
        sizeBytes: Long,
        sha256: String,
        rejectedSize: Long = -1,
        rejectedModified: Long = -1,
        checkCancelled: () -> Unit = {},
    ): File? {
        require(Regex("[a-f0-9]{64}").matches(sha256) && sizeBytes > 0) { "Invalid model identity." }
        val final = File(directory, "$sha256.gguf")
        if (!final.exists()) return null
        require(final.isFile && final.canonicalFile.parentFile == directory.canonicalFile &&
            final.canonicalFile.name == final.name) { "Model repair must stay within its download store." }
        val rejected = final.length() == rejectedSize && final.lastModified() == rejectedModified
        if (!rejected) {
            try {
                verify(final, sizeBytes, sha256, checkCancelled)
                return final
            } catch (_: IllegalArgumentException) {
                // Size/hash mismatch is repairable; IO/cancellation errors propagate.
            }
        }
        check(final.delete()) { "Could not remove the corrupt downloaded model." }
        return null
    }

    /** A DownloadManager row is disposable only with an exact owned partial path. */
    fun isUntrackedPartial(id: Long, trackedIds: Set<Long>, localUri: String?, directory: File): Boolean {
        if (id in trackedIds || localUri == null) return false
        return try {
            val uri = URI(localUri)
            if (uri.scheme != "file" || uri.rawAuthority != null || uri.rawQuery != null || uri.rawFragment != null) return false
            val destination = File(uri)
            val partialName = Regex("[a-f0-9]{64}\\.download")
            if (!partialName.matches(destination.name)) return false
            val file = destination.canonicalFile
            file.parentFile == directory.canonicalFile && partialName.matches(file.name)
        } catch (_: Exception) { false }
    }

    fun verify(file: File, sizeBytes: Long, sha256: String, checkCancelled: () -> Unit = {}) {
        require(sizeBytes > 0 && Regex("[a-f0-9]{64}").matches(sha256)) { "Invalid model identity." }
        require(file.isFile && file.length() == sizeBytes) { "Model size does not match the expected download." }
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().buffered().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                checkCancelled()
                val count = input.read(buffer)
                if (count < 0) break
                digest.update(buffer, 0, count)
            }
        }
        val actual = digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) }
        require(actual == sha256) { "Model checksum does not match. Retry the download." }
    }

    fun publish(partial: File, destination: File) {
        require(partial.canonicalFile.parentFile == destination.canonicalFile.parentFile) { "Model installation must remain on one filesystem." }
        check(!destination.exists()) { "A model already exists at the installation path." }
        Files.move(partial.toPath(), destination.toPath(), StandardCopyOption.ATOMIC_MOVE)
    }
}
