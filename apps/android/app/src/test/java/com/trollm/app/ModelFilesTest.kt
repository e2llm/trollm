package com.trollm.app

import java.io.File
import java.nio.file.Files
import java.security.MessageDigest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class ModelFilesTest {
    private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes)
        .joinToString("") { "%02x".format(it.toInt() and 255) }

    private fun withDirectory(test: (File) -> Unit) {
        val directory = Files.createTempDirectory("trollm-model-test").toFile()
        try { test(directory) } finally { directory.deleteRecursively() }
    }

    private fun rejects(block: () -> Unit) {
        try { block(); fail("Expected validation to reject the input") }
        catch (_: IllegalArgumentException) { }
        catch (_: IllegalStateException) { }
    }

    @Test fun orphanCleanupRecognizesOnlyUntrackedOwnedHashPartials() = withDirectory { directory ->
        val partial = File(directory, "${"a".repeat(64)}.download")
        assertTrue(ModelFiles.isUntrackedPartial(7, setOf(8), partial.toURI().toString(), directory))
        assertFalse(ModelFiles.isUntrackedPartial(7, setOf(7), partial.toURI().toString(), directory))
        assertFalse(ModelFiles.isUntrackedPartial(7, emptySet(), null, directory))
        assertFalse(ModelFiles.isUntrackedPartial(7, emptySet(), "content://downloads/7", directory))
        for (name in listOf("${"a".repeat(64)}.gguf", "coder.gguf", "model.download", "${"A".repeat(64)}.download")) {
            assertFalse(ModelFiles.isUntrackedPartial(7, emptySet(), File(directory, name).toURI().toString(), directory))
        }
        val outside = File(directory.parentFile, partial.name)
        assertFalse(ModelFiles.isUntrackedPartial(7, emptySet(), outside.toURI().toString(), directory))
        assertFalse(ModelFiles.isUntrackedPartial(7, emptySet(), "file://other-host${partial.absolutePath}", directory))
    }

    @Test fun verifiesBytesAndRejectsWrongSizeAndHash() = withDirectory { directory ->
        val bytes = ByteArray(200_000) { (it % 251).toByte() }
        val file = File(directory, "model.download").also { it.writeBytes(bytes) }
        ModelFiles.verify(file, bytes.size.toLong(), sha(bytes))
        rejects { ModelFiles.verify(file, bytes.size.toLong() - 1, sha(bytes)) }
        rejects { ModelFiles.verify(file, bytes.size.toLong(), "0".repeat(64)) }
        rejects { ModelFiles.verify(File(directory, "missing"), bytes.size.toLong(), sha(bytes)) }
        rejects { ModelFiles.verify(file, bytes.size.toLong(), "../invalid") }
    }

    @Test fun cancellationStopsStreamingAndLeavesOriginalUntouched() = withDirectory { directory ->
        val bytes = ByteArray(200_000) { 1 }
        val partial = File(directory, "model.download").also { it.writeBytes(bytes) }
        var checks = 0
        try {
            ModelFiles.verify(partial, bytes.size.toLong(), sha(bytes)) {
                if (++checks == 2) throw InterruptedException("cancelled")
            }
            fail("Expected cancellation")
        } catch (_: InterruptedException) { }
        assertEquals(2, checks)
        assertEquals(bytes.size.toLong(), partial.length())
        assertFalse(File(directory, "model.gguf").exists())
    }

    @Test fun atomicPublicationMovesWithoutRetainingSecondCopy() = withDirectory { directory ->
        val partial = File(directory, "model.download").also { it.writeText("verified") }
        val final = File(directory, "model.gguf")
        ModelFiles.publish(partial, final)
        assertFalse(partial.exists())
        assertEquals("verified", final.readText())
    }

    @Test fun explicitRetryRepairsOnlyCorruptOwnedFinalAndAdoptsValidBytes() = withDirectory { directory ->
        val bytes = "verified model".toByteArray()
        val hash = sha(bytes)
        val final = File(directory, "$hash.gguf").also { it.writeBytes(bytes) }
        val legacy = File(directory, "coder.gguf").also { it.writeText("legacy untouched") }
        assertEquals(final, ModelFiles.repairFinal(directory, bytes.size.toLong(), hash))
        assertTrue(final.exists())
        final.writeText("corrupt")
        val rejectedSize = final.length()
        val rejectedModified = final.lastModified()
        assertEquals(null, ModelFiles.repairFinal(directory, bytes.size.toLong(), hash, rejectedSize, rejectedModified))
        assertFalse(final.exists())
        assertEquals("legacy untouched", legacy.readText())
        // Changed metadata invalidates a prior rejection: valid replacement is adopted.
        final.writeBytes(bytes)
        assertEquals(final, ModelFiles.repairFinal(directory, bytes.size.toLong(), hash, rejectedSize, rejectedModified))
        assertTrue(final.exists())
        final.writeText("another corrupt model")
        assertEquals(null, ModelFiles.repairFinal(directory, bytes.size.toLong(), hash))
        assertFalse(final.exists())
        assertTrue(legacy.exists())
    }

    @Test fun publicationRefusesExistingFinalAndDifferentDirectory() = withDirectory { directory ->
        val partial = File(directory, "model.download").also { it.writeText("new") }
        val final = File(directory, "model.gguf").also { it.writeText("existing") }
        rejects { ModelFiles.publish(partial, final) }
        assertEquals("existing", final.readText())
        assertTrue(partial.exists())
        val other = File(directory, "other").also { it.mkdir() }
        rejects { ModelFiles.publish(partial, File(other, "model.gguf")) }
        assertTrue(partial.exists())
    }
}
