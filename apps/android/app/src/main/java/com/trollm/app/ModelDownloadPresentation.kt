package com.trollm.app

import java.util.Locale

/** Decimal units match model publishers; presentation never changes saved byte counts. */
fun modelByteSize(bytes: Long): String {
    val count = bytes.coerceAtLeast(0)
    val units = arrayOf("B", "kB", "MB", "GB", "TB", "PB", "EB")
    var value = count.toDouble()
    var unit = 0
    while (value >= 1000 && unit < units.lastIndex) {
        value /= 1000
        unit++
    }
    if (unit == 0) return "$count B"
    val decimals = if (value >= 100) 0 else if (value >= 10) 1 else 2
    return String.format(Locale.ROOT, "%.${decimals}f %s", value, units[unit])
}

/** Null means no usable expected total. Double avoids Long overflow. */
fun modelDownloadFraction(downloaded: Long, total: Long): Float? =
    if (total <= 0) null else (downloaded.coerceAtLeast(0).toDouble() / total.toDouble()).coerceIn(0.0, 1.0).toFloat()

fun modelDownloadPercent(downloaded: Long, total: Long): Int? = when {
    total <= 0 -> null
    downloaded >= total -> 100
    // Floating-point rounding must not label an incomplete transfer as 100%.
    else -> ((downloaded.coerceAtLeast(0).toDouble() / total.toDouble()) * 100).toInt().coerceIn(0, 99)
}

/** Saturate unexpected oversized aggregates instead of displaying a negative total. */
fun modelDownloadTotal(sizes: Iterable<Long>): Long = sizes.fold(0L) { total, size ->
    val positive = size.coerceAtLeast(0)
    if (positive > Long.MAX_VALUE - total) Long.MAX_VALUE else total + positive
}


internal const val MODEL_SPACE_HEADROOM = 256L * 1024 * 1024

/** Display the next file's admission threshold separately from the total transfer. */
internal fun modelNextFileFreeSpace(size: Long): Long =
    modelDownloadTotal(listOf(size, MODEL_SPACE_HEADROOM))

enum class ModelTransferPhase { QUEUED, WAITING_FOR_NETWORK, PAUSED, TRANSFERRING }
