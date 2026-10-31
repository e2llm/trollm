package com.trollm.app

import org.json.JSONObject
import java.io.File
import java.net.URI
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest
import java.time.Instant
import javax.net.ssl.HttpsURLConnection

/** Source authentication never enters the MCP token provider or runtime config. */
data class MorayProfile(val api: String, val issuer: String, val clientId: String, val consumerRevision: String) {
    init {
        for (value in listOf(api, issuer)) {
            require(ConnectionInput.canonicalEndpoint(value) == value && URI(value).rawQuery == null)
            require(!URI(value).rawPath.split('/').any { it == "." || it == ".." })
        }
        require(api.endsWith("/v1/portable-publications") && !issuer.endsWith('/'))
        require(consumerRevision.matches(Regex("[a-f0-9]{40}")))
        require(clientId.matches(Regex("[A-Za-z0-9_-]{1,128}")))
    }
    val key: String get() = MorayPortable.sha("$api\n$issuer\n$clientId".toByteArray())
}

class MorayFailure(val code: String) : IllegalStateException(code)
class MorayResponse(val status: Int, val headers: Map<String, String>, val bytes: ByteArray)
fun interface MorayWire {
    fun request(method: String, url: String, token: String, body: ByteArray?, limit: Int): MorayResponse
}

/** TLS plus operator corporate roots, platform hostname verification, no redirects. */
class MorayHttps(private val filesDir: File) : MorayWire {
    override fun request(method: String, url: String, token: String, body: ByteArray?, limit: Int): MorayResponse {
        require(ConnectionInput.validToken(token))
        val connection = URI(ConnectionInput.canonicalEndpoint(url)).toURL().openConnection() as HttpsURLConnection
        OperatorTrustRoots.socketFactory(File(filesDir, "trollm/trust/moray-roots.pem").toPath())
            ?.let { connection.sslSocketFactory = it }
        connection.instanceFollowRedirects = false
        connection.connectTimeout = 15_000
        connection.readTimeout = 15_000
        connection.requestMethod = method
        connection.setRequestProperty("Authorization", "Bearer $token")
        connection.setRequestProperty("Accept", if (url.endsWith("/bundle")) "application/x-tar" else "application/json")
        try {
            if (body != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.setFixedLengthStreamingMode(body.size)
                connection.outputStream.use { it.write(body) }
            }
            val status = connection.responseCode
            // Error bodies can contain private diagnostics. Retain only HTTP status.
            val bytes = if (status in 200..299) connection.inputStream.use { BundleInput.read(it, limit) } else byteArrayOf()
            return MorayResponse(status, connection.headerFields.filterKeys { it != null }
                .mapKeys { it.key.lowercase() }.mapValues { it.value.joinToString(",") }, bytes)
        } finally { connection.disconnect() }
    }
}

/** One immutable owned publication, dependency-complete install and report outbox.
 * No method executes a flow or retries a model/browser action.
 */
class MorayPortable(
    private val profile: MorayProfile,
    private val directory: File,
    private val wire: MorayWire,
    private val token: () -> String?,
    private val grantId: String,
    private val now: () -> Instant = Instant::now,
) {
    private val lock = locks.computeIfAbsent(directory.absolutePath) { Any() }
    init { require(grantId.isNotBlank()); directory.mkdirs(); require(directory.isDirectory) }

    private fun request(method: String, path: String, body: ByteArray? = null, limit: Int = 1024 * 1024): MorayResponse {
        val access = token()?.takeIf(ConnectionInput::validToken) ?: throw MorayFailure("SOURCE_RECONNECT_REQUIRED")
        val response = try { wire.request(method, profile.api + path, access, body, limit) }
            catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (safe: MorayFailure) { throw safe }
            catch (_: Exception) { throw MorayFailure("SOURCE_TRANSPORT_UNAVAILABLE") }
        if (response.status != if (method == "POST") 201 else 200) throw MorayFailure("SOURCE_HTTP_${response.status}")
        require(response.bytes.size <= limit)
        if (token()?.takeIf(ConnectionInput::validToken) == null) throw MorayFailure("SOURCE_RECONNECT_REQUIRED")
        return response
    }

    fun descriptor(publicationId: String): JSONObject {
        id(publicationId)
        val value = json(request("GET", "/$publicationId").bytes)
        closed(value, "schema", "publicationId", "source", "identity", "metadata", "consumerRevision", "createdAt", "expiresAt", "availability", "compatibility")
        require(value.sourceString("schema") == "moray.portable-publication.v1" && value.sourceString("publicationId") == publicationId)
        val source = value.getJSONObject("source")
        val declared = source.opt("origin") == "DECLARED"
        if (declared) {
            closed(source, "schemaVersion", "origin", "owner", "fixtureId", "versionId", "fixtureSha256", "promptSha256", "installationSha256", "qualification")
            require(source.sourceString("schemaVersion") == "moray.declared-native-text.v1")
            id(source.sourceString("fixtureId"))
            for (name in listOf("fixtureSha256", "promptSha256", "installationSha256")) hash(source.sourceString(name))
        } else closed(source, "owner", "versionId", "planSha256", "qualification", "sourceFindings", "provenance", "findings")
        val owner = source.getJSONObject("owner")
        closed(owner, "registrationId", "tenantId", "subject", "workerId", "runId", "epoch")
        for (name in listOf("registrationId", "tenantId", "subject", "workerId", "runId")) id(owner.sourceString(name))
        val epoch = owner.get("epoch")
        require(epoch is Number && epoch.toString().matches(Regex("[1-9][0-9]*")) && epoch.toLong() <= 9007199254740991L)
        id(source.sourceString("versionId"))
        if (!declared) hash(source.sourceString("planSha256"))
        require(source.sourceString("qualification") == "unqualified")
        val unresolvedRevision = value.isNull("consumerRevision")
        if (unresolvedRevision) require(declared) else
            require(value.sourceString("consumerRevision").matches(Regex("[a-f0-9]{40}")))
        val created = Instant.parse(value.sourceString("createdAt"))
        val expires = Instant.parse(value.sourceString("expiresAt"))
        require(expires > created)
        val availability = value.getJSONObject("availability")
        closed(availability, "available", "reason")
        val compatibility = value.getJSONObject("compatibility")
        closed(compatibility, "status", "reasons")
        if (unresolvedRevision) {
            require(availability.get("available") == false && availability.sourceString("reason") == "NO_EXPORT" &&
                compatibility.sourceString("status") == "unsupported" && value.isNull("identity") && value.isNull("metadata"))
        }
        if (expires <= now()) throw MorayFailure("PUBLICATION_EXPIRED")
        if (availability.get("available") != true) {
            val reason = availability.sourceString("reason")
            require(reason in listOf("NO_EXPORT", "PUBLICATION_REVOKED", "PUBLICATION_EXPIRED"))
            throw MorayFailure(reason)
        }
        require(availability.isNull("reason"))
        if (compatibility.sourceString("status") != "supported") throw MorayFailure("NO_EXPORT")
        require(compatibility.getJSONArray("reasons").length() == 0)
        if (value.sourceString("consumerRevision") != profile.consumerRevision) throw MorayFailure("CONSUMER_REVISION_UNSUPPORTED")
        val identity = value.getJSONObject("identity")
        closed(identity, "format", "sha256", "byteSize", "manifestSha256", "entryFlowSha256")
        require(identity.sourceString("format") == "trollm.flow-bundle.v1")
        for (name in listOf("sha256", "manifestSha256", "entryFlowSha256")) hash(identity.sourceString(name))
        val size = identity.get("byteSize")
        require(size is Number && size.toString().matches(Regex("[1-9][0-9]*")) && size.toLong() <= BundleInput.MAX_BYTES)
        value.getJSONObject("metadata")
        return value
    }

    /** install verifies every archive member using the existing native importer. */
    fun pull(publicationId: String, install: (ByteArray) -> JSONObject): JSONObject = synchronized(lock) {
        val descriptor = descriptor(publicationId)
        val identity = descriptor.getJSONObject("identity")
        val address = "$publicationId-${identity.sourceString("sha256")}"
        val stateFile = File(directory, "$address.json")
        val archiveFile = File(directory, "$address.tar")
        if (stateFile.exists()) {
            val saved = json(stateFile.readBytes())
            require(saved.sourceString("grantId") == grantId && saved.sourceString("profile") == profile.key)
            require(JsonValues.same(saved.getJSONObject("descriptor"), descriptor)) { "PUBLICATION_IDENTITY_CHANGED" }
        }
        // Preserve original descriptor before any transfer/install side effect.
        val state = JSONObject().put("profile", profile.key).put("grantId", grantId)
            .put("profileConfig", JSONObject().put("api", profile.api).put("issuer", profile.issuer)
                .put("clientId", profile.clientId).put("consumerRevision", profile.consumerRevision))
            .put("descriptor", descriptor)
        atomic(stateFile, state.toString().toByteArray())
        val bytes = if (archiveFile.exists()) archiveFile.readBytes() else {
            val response = request("GET", "/$publicationId/bundle", limit = BundleInput.MAX_BYTES)
            require(response.headers["content-type"]?.substringBefore(';') == "application/x-tar")
            require(response.headers["content-length"]?.toLongOrNull() == identity.getLong("byteSize"))
            require(response.headers["x-content-sha256"] == identity.sourceString("sha256"))
            response.bytes
        }
        require(bytes.size.toLong() == identity.getLong("byteSize") && sha(bytes) == identity.sourceString("sha256"))
        // Current source availability/owner and original deadline, not a restamp.
        require(JsonValues.same(descriptor(publicationId), descriptor)) { "PUBLICATION_IDENTITY_CHANGED" }
        atomic(archiveFile, bytes)
        val installed = install(bytes)
        require(installed.sourceString("archiveSha256") == identity.sourceString("sha256"))
        require(installed.sourceString("installationSha256") == identity.sourceString("manifestSha256"))
        require(installed.sourceString("flowSha256") == identity.sourceString("entryFlowSha256"))
        state.put("installation", installed)
        atomic(stateFile, state.toString().toByteArray())
        state
    }

    /** Caller supplies actual retained run/attempt/items; no success is inferred. */
    fun retainReport(installation: JSONObject, report: JSONObject): String = synchronized(lock) {
        require(installation.sourceString("profile") == profile.key && installation.sourceString("grantId") == grantId)
        val descriptor = installation.getJSONObject("descriptor")
        require(installation.has("installation"))
        validateReport(report)
        require(report.sourceString("publicationId") == descriptor.sourceString("publicationId") &&
            report.sourceString("bundleSha256") == descriptor.getJSONObject("identity").sourceString("sha256"))
        val key = report.sourceString("reportId")
        val file = File(directory, "report-$key.json")
        val pending = JSONObject().put("profile", profile.key).put("grantId", grantId).put("installation", installation).put("report", report)
        if (file.exists()) {
            val original = json(file.readBytes())
            require(JsonValues.same(original.getJSONObject("report"), report) && JsonValues.same(original.getJSONObject("installation"), installation) && original.sourceString("grantId") == grantId) { "REPORT_IDENTITY_CONFLICT" }
        }
        else atomic(file, pending.toString().toByteArray())
        key
    }

    /** Retry the exact original report, never the execution that produced it. */
    fun sendReport(reportId: String): JSONObject = synchronized(lock) {
        id(reportId)
        val file = File(directory, "report-$reportId.json")
        val pending = json(file.readBytes())
        require(pending.sourceString("profile") == profile.key && pending.sourceString("grantId") == grantId)
        val report = pending.getJSONObject("report")
        validateReport(report)
        val path = "/${report.sourceString("publicationId")}/reports"
        val response = request("POST", path, report.toString().toByteArray())
        val receipt = json(response.bytes)
        closed(receipt, "schema", "report", "receivedAt", "verification", "reporter")
        require(receipt.sourceString("schema") == "moray.edge-report.v1" && receipt.sourceString("verification") == "device-reported")
        require(JsonValues.same(receipt.getJSONObject("report"), report)) { "REPORT_RECEIPT_MISMATCH" }
        Instant.parse(receipt.sourceString("receivedAt"))
        val reporter = receipt.getJSONObject("reporter")
        closed(reporter, "kind", "issuer", "subject", "clientId")
        require(reporter.sourceString("kind") == "native" && reporter.sourceString("issuer") == profile.issuer && reporter.sourceString("clientId") == profile.clientId)
        require(reporter.sourceString("subject").length in 1..256)
        val receiptFile = File(directory, "receipt-$reportId.json")
        if (receiptFile.exists()) require(JsonValues.same(json(receiptFile.readBytes()), receipt)) { "REPORT_RECEIPT_CHANGED" }
        else atomic(receiptFile, response.bytes)
        receipt
    }

    companion object {
        private val locks = java.util.concurrent.ConcurrentHashMap<String, Any>()
        fun sha(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        private fun id(value: String) = require(value.matches(Regex("[A-Za-z0-9][A-Za-z0-9_-]{0,127}")))
        private fun hash(value: String) = require(value.matches(Regex("[a-f0-9]{64}")))
        private fun json(bytes: ByteArray): JSONObject = JSONObject(Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(bytes)).toString())
        private fun closed(value: JSONObject, vararg keys: String) = require(JsonValues.keys(value) == keys.toSet())
        internal fun atomic(file: File, bytes: ByteArray) {
            val temporary = Files.createTempFile(file.parentFile.toPath(), ".moray-", ".tmp")
            try {
                java.io.FileOutputStream(temporary.toFile()).use { it.write(bytes); it.fd.sync() }
                Files.move(temporary, file.toPath(), StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING)
            } finally { Files.deleteIfExists(temporary) }
        }
        private fun validateReport(report: JSONObject) {
            closed(report, "reportId", "deviceId", "runId", "attemptId", "publicationId", "bundleSha256", "outcome", "items")
            for (name in listOf("reportId", "deviceId", "runId", "attemptId", "publicationId")) id(report.sourceString(name))
            hash(report.sourceString("bundleSha256"))
            require(report.sourceString("outcome") in listOf("partial", "unknown", "failed", "reported-complete"))
            val items = report.getJSONArray("items"); require(items.length() <= 128)
            val ids = mutableSetOf<String>()
            for (index in 0 until items.length()) {
                val item = items.getJSONObject(index)
                closed(item, "itemId", "classification", "evidenceSha256")
                val key = item.sourceString("itemId"); id(key); require(ids.add(key))
                require(item.sourceString("classification") in listOf("not-started", "in-flight", "outcome-unknown", "observed", "verified"))
                val hashes = item.getJSONArray("evidenceSha256"); require(hashes.length() <= 32)
                for (j in 0 until hashes.length()) hash(hashes.sourceString(j))
            }
        }
    }
}
