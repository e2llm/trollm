package com.trollm.app

import android.net.Uri
import net.openid.appauth.connectivity.ConnectionBuilder
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import javax.net.ssl.HttpsURLConnection

class OAuthConnectionBuilder(filesDir: File) : ConnectionBuilder {
    private val roots = File(filesDir, "trollm/trust/mcp-roots.pem").toPath()

    override fun openConnection(uri: Uri): HttpURLConnection {
        val endpoint = ConnectionInput.canonicalEndpoint(uri.toString())
        val connection = URI(endpoint).toURL().openConnection() as HttpsURLConnection
        // Keep the platform hostname verifier. Additional roots never bypass it.
        OperatorTrustRoots.socketFactory(roots)?.let { connection.sslSocketFactory = it }
        connection.connectTimeout = 15_000
        connection.readTimeout = 15_000
        connection.instanceFollowRedirects = false
        return connection
    }
}
