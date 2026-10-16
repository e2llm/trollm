package com.trollm.app

import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

class OperatorTrustRootsTest {
    @Test fun absenceUsesSystemTrustAndPresentInvalidFilesFailClosed() {
        val dir = Files.createTempDirectory("trollm-trust-test")
        val path = dir.resolve("mcp-roots.pem")
        try {
            assertNull(OperatorTrustRoots.socketFactory(path))
            for (invalid in listOf(byteArrayOf(), "invalid".toByteArray(),
                "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n".toByteArray(),
                ByteArray(128 * 1024 + 1))) {
                Files.write(path, invalid)
                try { OperatorTrustRoots.socketFactory(path); fail("invalid roots accepted") }
                catch (_: Exception) { }
            }
            Files.delete(path)
            Files.createDirectory(path)
            try { OperatorTrustRoots.socketFactory(path); fail("directory accepted") }
            catch (_: IllegalArgumentException) { }
            Files.delete(path)
            val target = dir.resolve("other")
            Files.write(target, "invalid".toByteArray())
            Files.createSymbolicLink(path, target)
            try { OperatorTrustRoots.socketFactory(path); fail("symlink accepted") }
            catch (_: IllegalArgumentException) { }
            Files.delete(path)
            Files.delete(target)
        } finally {
            Files.deleteIfExists(path)
            Files.deleteIfExists(dir)
        }
    }
    @Test fun validSyntheticOperatorRootCanAugmentSystemTrust() {
        val path = Files.createTempFile("trollm-public-test-root", ".crt")
        try {
            Files.write(path, TEST_ROOT.toByteArray())
            assertNotNull(OperatorTrustRoots.socketFactory(path))
        } finally { Files.deleteIfExists(path) }
    }

    companion object {
        // Public synthetic certificate from the MCP loopback fixture; no deployment trust.
        private val TEST_ROOT = """
            -----BEGIN CERTIFICATE-----
            MIIBWzCCAQGgAwIBAgIBATAKBggqhkjOPQQDAjAiMSAwHgYDVQQDDBd0cm9sbG0g
            dGVzdC1vbmx5IHJvb3QgMTAgFw0yMDAxMDEwMDAwMDBaGA8yMTIwMDEwMTAwMDAw
            MFowIjEgMB4GA1UEAwwXdHJvbGxtIHRlc3Qtb25seSByb290IDEwWTATBgcqhkjO
            PQIBBggqhkjOPQMBBwNCAAT0dOsi/OBTc8/IUhDDQxwVWt81yyRer8Q8Mp/gexDy
            87uqS8pRBBSN5lf9tS0mt0DUc0sEJuVCrenfUzv2MULBoyYwJDASBgNVHRMBAf8E
            CDAGAQH/AgEAMA4GA1UdDwEB/wQEAwIBBjAKBggqhkjOPQQDAgNIADBFAiEA++3G
            lrYdCxyXoKKkzts2+t09N8Ln8e0ws8XgzNU5aYECIHcnhEZbbGc3vvCKOGNicrEC
            pg/MdKffm9ZUGuyx0lWR
            -----END CERTIFICATE-----
        """.trimIndent()
    }
}
