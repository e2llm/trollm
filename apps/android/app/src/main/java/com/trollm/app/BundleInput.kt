package com.trollm.app

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream

/** Bound document-provider reads before allocating the UniFFI bundle argument. */
object BundleInput {
    // Keep in sync with crates/trollm-uniffi/src/tar.rs::MAX_BUNDLE_BYTES.
    const val MAX_BYTES = 64 * 1024 * 1024

    internal fun read(input: InputStream, maxBytes: Int = MAX_BYTES): ByteArray {
        require(maxBytes >= 0 && maxBytes < Int.MAX_VALUE)
        val output = ByteArrayOutputStream(minOf(maxBytes, 8192))
        val buffer = ByteArray(8192)
        var total = 0
        while (true) {
            // Read one byte beyond the bound to distinguish an exact-size file from overflow.
            val count = input.read(buffer, 0, minOf(buffer.size, maxBytes - total + 1))
            if (count == -1) return output.toByteArray()
            if (count == 0) throw IOException("Selected file could not be read")
            if (count > maxBytes - total) {
                throw IOException("Bundle exceeds $maxBytes bytes")
            }
            output.write(buffer, 0, count)
            total += count
        }
    }
}
