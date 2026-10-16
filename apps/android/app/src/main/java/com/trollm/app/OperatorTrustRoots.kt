package com.trollm.app

import java.io.ByteArrayInputStream
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Path
import java.nio.file.attribute.BasicFileAttributes
import java.security.KeyStore
import java.security.cert.CertificateException
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocketFactory
import javax.net.ssl.TrustManagerFactory
import javax.net.ssl.X509TrustManager

/** Same operator-only PEM path/policy as Rust. No bundled or endpoint-selected roots. */
object OperatorTrustRoots {
    private const val MAX_BYTES = 128 * 1024

    fun socketFactory(path: Path): SSLSocketFactory? {
        val metadata = try {
            Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
        } catch (_: java.nio.file.NoSuchFileException) {
            return null
        }
        require(metadata.isRegularFile) { "MCP trust must be a regular PEM file" }
        val bytes = Files.newInputStream(path).use { input ->
            val buffer = ByteArray(MAX_BYTES + 1)
            var size = 0
            while (size < buffer.size) {
                val read = input.read(buffer, size, buffer.size - size)
                if (read == -1) break
                size += read
            }
            require(size <= MAX_BYTES) { "MCP trust exceeds 128 KiB" }
            buffer.copyOf(size)
        }
        require(bytes.toString(Charsets.US_ASCII).contains("-----BEGIN CERTIFICATE-----")) {
            "MCP trust contains no PEM certificates"
        }
        val certificates = CertificateFactory.getInstance("X.509")
            .generateCertificates(ByteArrayInputStream(bytes)).map { it as X509Certificate }
        require(certificates.isNotEmpty()) { "MCP trust contains no certificates" }
        val extraStore = KeyStore.getInstance(KeyStore.getDefaultType()).apply {
            load(null)
            certificates.forEachIndexed { index, cert -> setCertificateEntry("operator-$index", cert) }
        }
        val system = trustManager(null)
        val extra = trustManager(extraStore)
        val combined = object : X509TrustManager {
            override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                try { system.checkServerTrusted(chain, authType) }
                catch (_: CertificateException) { extra.checkServerTrusted(chain, authType) }
            }
            override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
                try { system.checkClientTrusted(chain, authType) }
                catch (_: CertificateException) { extra.checkClientTrusted(chain, authType) }
            }
            override fun getAcceptedIssuers(): Array<X509Certificate> =
                system.acceptedIssuers + extra.acceptedIssuers
        }
        return SSLContext.getInstance("TLS").apply {
            init(null, arrayOf(combined), null)
        }.socketFactory
    }

    private fun trustManager(store: KeyStore?): X509TrustManager =
        TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply {
            init(store)
        }.trustManagers.filterIsInstance<X509TrustManager>().first()
}
