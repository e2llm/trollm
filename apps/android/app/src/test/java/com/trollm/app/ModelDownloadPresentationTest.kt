package com.trollm.app

import java.util.Locale
import org.junit.Assert.*
import org.junit.Test

class ModelDownloadPresentationTest {
    @Test fun sizesUseDecimalUnitsAndStayReadableAcrossMagnitudes() {
        assertEquals("0 B", modelByteSize(-1))
        assertEquals("999 B", modelByteSize(999))
        assertEquals("1.00 kB", modelByteSize(1000))
        assertEquals("533 MB", modelByteSize(533_000_000))
        assertEquals("3.11 GB", modelByteSize(3_110_000_000))
        assertEquals("9.22 EB", modelByteSize(Long.MAX_VALUE))
    }

    @Test fun sizesDoNotDependOnDeviceNumberLocale() {
        val original = Locale.getDefault()
        try {
            Locale.setDefault(Locale.GERMANY)
            assertEquals("3.11 GB", modelByteSize(3_110_000_000))
        } finally { Locale.setDefault(original) }
    }

    @Test fun unknownTotalsRemainIndeterminate() {
        assertNull(modelDownloadFraction(100, 0))
        assertNull(modelDownloadPercent(100, -1))
    }

    @Test fun progressIsClampedAndLargeValuesDoNotOverflow() {
        assertEquals(0, modelDownloadPercent(-1, 100))
        assertEquals(46, modelDownloadPercent(1_430_600_000, 3_110_000_000))
        assertEquals(100, modelDownloadPercent(Long.MAX_VALUE, 1))
        assertEquals(100, modelDownloadPercent(Long.MAX_VALUE, Long.MAX_VALUE))
        assertEquals(99, modelDownloadPercent(Long.MAX_VALUE - 1, Long.MAX_VALUE))
        assertEquals(0.5f, modelDownloadFraction(Long.MAX_VALUE / 2, Long.MAX_VALUE)!!, 0.0001f)
        assertEquals(1f, modelDownloadFraction(101, 100)!!, 0f)
    }

    @Test fun combinedSizeSaturatesInsteadOfWrapping() {
        assertEquals(0L, modelDownloadTotal(emptyList()))
        assertEquals(300L, modelDownloadTotal(listOf(100, 200, -1)))
        assertEquals(Long.MAX_VALUE, modelDownloadTotal(listOf(Long.MAX_VALUE, 1)))
    }
}
