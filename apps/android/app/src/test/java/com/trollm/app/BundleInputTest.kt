package com.trollm.app

import java.io.ByteArrayInputStream
import java.io.IOException
import java.io.InputStream
import org.junit.Assert.*
import org.junit.Test

class BundleInputTest {
    @Test fun exactLimitIsAcceptedAndPreservesBytes() {
        val bytes = ByteArray(16_384) { (it % 251).toByte() }
        assertArrayEquals(bytes, BundleInput.read(ByteArrayInputStream(bytes), bytes.size))
    }

    @Test fun oversizedProviderStopsAfterOneByteBeyondLimit() {
        var consumed = 0
        val endless = object : InputStream() {
            override fun read(): Int { consumed++; return 42 }
            override fun read(bytes: ByteArray, offset: Int, length: Int): Int {
                bytes.fill(42, offset, offset + length)
                consumed += length
                return length
            }
        }
        val error = assertThrows(IOException::class.java) { BundleInput.read(endless, 17) }
        assertEquals("Bundle exceeds 17 bytes", error.message)
        assertEquals(18, consumed)
    }

    @Test fun shortProviderReadsDoNotTruncateTheBundle() {
        val bytes = byteArrayOf(0, 1, -1, 42, 64)
        val shortReads = object : ByteArrayInputStream(bytes) {
            override fun read(buffer: ByteArray, offset: Int, length: Int): Int =
                super.read(buffer, offset, minOf(2, length))
        }
        assertArrayEquals(bytes, BundleInput.read(shortReads, bytes.size))
    }

    @Test fun providerFailureIsPropagatedAndUseClosesTheStream() {
        val failure = IOException("provider disconnected")
        var closed = false
        val input = object : InputStream() {
            override fun read(): Int = throw failure
            override fun close() { closed = true }
        }
        val actual = assertThrows(IOException::class.java) {
            input.use { BundleInput.read(it, 8) }
        }
        assertSame(failure, actual)
        assertTrue(closed)
    }

    @Test fun emptyInputReachesTheRuntimeForFormatValidation() {
        assertArrayEquals(byteArrayOf(), BundleInput.read(ByteArrayInputStream(byteArrayOf()), 0))
    }

    @Test fun stalledProviderFailsInsteadOfSpinning() {
        val input = object : InputStream() {
            override fun read(): Int = 0
            override fun read(bytes: ByteArray, offset: Int, length: Int): Int = 0
        }
        assertThrows(IOException::class.java) { BundleInput.read(input, 8) }
    }
}
