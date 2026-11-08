package com.trollm.app

import org.json.JSONObject
import org.json.JSONArray
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files
import java.time.Instant

class MorayPortableTest {
    private val profile = MorayProfile("https://source.test/v1/portable-publications", "https://login.test/realms/source", "native-public", "d".repeat(40))
    private val clock = Instant.parse("2026-10-30T12:00:00Z")
    private val bytes = "controlled archive passed to native installer".toByteArray()
    private val archive = MorayPortable.sha(bytes)
    private val manifest = "a".repeat(64)
    private val root = "b".repeat(64)
    private fun descriptor() = JSONObject().put("schema", "moray.portable-publication.v1").put("publicationId", "publication1")
        .put("source", JSONObject().put("owner", JSONObject().put("registrationId", "reg").put("tenantId", "tenant")
            .put("subject", "subject").put("workerId", "worker").put("runId", "source-run").put("epoch", 1))
            .put("versionId", "version").put("planSha256", "c".repeat(64)).put("qualification", "unqualified")
            .put("sourceFindings", JSONArray()).put("findings", JSONArray()).put("provenance", JSONObject()))
        .put("identity", JSONObject().put("format", "trollm.flow-bundle.v1").put("sha256", archive).put("byteSize", bytes.size)
            .put("manifestSha256", manifest).put("entryFlowSha256", root))
        .put("metadata", JSONObject()).put("consumerRevision", "d".repeat(40))
        .put("createdAt", "2026-10-30T11:00:00Z").put("expiresAt", "2026-10-30T13:00:00Z")
        .put("availability", JSONObject().put("available", true).put("reason", JSONObject.NULL))
        .put("compatibility", JSONObject().put("status", "supported").put("reasons", JSONArray()))
    private fun installation() = JSONObject().put("archiveSha256", archive).put("installationSha256", manifest).put("flowSha256", root)
    private fun report() = JSONObject().put("reportId", "report1").put("deviceId", "device").put("runId", "native-run")
        .put("attemptId", "attempt1").put("publicationId", "publication1").put("bundleSha256", archive)
        .put("outcome", "unknown").put("items", JSONArray().put(JSONObject().put("itemId", "item1")
            .put("classification", "outcome-unknown").put("evidenceSha256", JSONArray())))
    private fun json(value: JSONObject, status: Int = 200) = MorayResponse(status, emptyMap(), value.toString().toByteArray())
    private fun bundle() = MorayResponse(200, mapOf("content-type" to "application/x-tar", "content-length" to bytes.size.toString(), "x-content-sha256" to archive), bytes)
    private fun reject(work: () -> Unit) { try { work(); fail("accepted invalid source data") } catch (_: IllegalArgumentException) {} catch (_: MorayFailure) {} }

    @Test fun unsupportedExpiredOrRevokedNeverDownloadsOrInstalls() {
        val variants = listOf(
            descriptor().put("expiresAt", "2026-10-30T11:59:00Z"),
            descriptor().put("availability", JSONObject().put("available", false).put("reason", "NO_EXPORT")).put("identity", JSONObject.NULL),
            descriptor().put("availability", JSONObject().put("available", false).put("reason", "PUBLICATION_REVOKED")),
            descriptor().put("consumerRevision", "e".repeat(40)),
            descriptor().put("compatibility", JSONObject().put("status", "unsupported").put("reasons", JSONArray().put("UNKNOWN"))),
        )
        for (value in variants) {
            val dir = Files.createTempDirectory("moray-refusal").toFile()
            try {
                var reads = 0
                val client = MorayPortable(profile, dir, MorayWire { _, url, _, _, _ ->
                    assertFalse(url.endsWith("/bundle")); reads++; json(value)
                }, { "source-token" }, "grant1", { clock })
                reject { client.pull("publication1") { fail("installer called"); installation() } }
                assertEquals(1, reads)
            } finally { dir.deleteRecursively() }
        }
    }

    @Test fun transferLossReusesOriginalArchiveAndNativeInstallerRatherThanExecution() {
        val dir = Files.createTempDirectory("moray-transfer").toFile()
        try {
            var transfers = 0
            val wire = MorayWire { _, url, _, _, _ -> if (url.endsWith("/bundle")) { transfers++; bundle() } else json(descriptor()) }
            var installs = 0
            val client = MorayPortable(profile, dir, wire, { "source-token" }, "grant1", { clock })
            try { client.pull("publication1") { installs++; throw IllegalStateException("interrupted install") }; fail() }
            catch (_: IllegalStateException) {}
            val saved = client.pull("publication1") { installs++; installation() }
            assertEquals(1, transfers)
            assertEquals(2, installs)
            assertEquals(archive, saved.getJSONObject("installation").getString("archiveSha256"))
            // Idempotent native reinspection also catches later installed-file drift.
            client.pull("publication1") { installs++; installation() }
            assertEquals(3, installs)
            assertEquals(1, transfers)
        } finally { dir.deleteRecursively() }
    }

    @Test fun wrongBytesHeadersOwnershipOrPostTransferRevocationRefuses() {
        for (mode in 0..4) {
            val dir = Files.createTempDirectory("moray-drift").toFile()
            try {
                var descriptions = 0
                val client = MorayPortable(profile, dir, MorayWire { _, url, _, _, _ ->
                    if (url.endsWith("/bundle")) when (mode) {
                        0 -> MorayResponse(200, bundle().headers, byteArrayOf(1))
                        1 -> MorayResponse(200, bundle().headers + ("x-content-sha256" to "f".repeat(64)), bytes)
                        else -> bundle()
                    } else {
                        descriptions++
                        val d = descriptor()
                        if (descriptions > 1 && mode == 2) d.getJSONObject("source").getJSONObject("owner").put("epoch", 2)
                        if (descriptions > 1 && mode == 3) d.getJSONObject("availability").put("available", false).put("reason", "PUBLICATION_REVOKED")
                        if (mode == 4) d.put("unexpected", true)
                        json(d)
                    }
                }, { "source-token" }, "grant1", { clock })
                reject { client.pull("publication1") { fail("invalid archive installed"); installation() } }
            } finally { dir.deleteRecursively() }
        }
    }

    @Test fun durableReportLossRetriesOnlyExactReportAndKeepsUnknown() {
        val dir = Files.createTempDirectory("moray-report").toFile()
        try {
            val submitted = mutableListOf<String>()
            val original = report()
            var failReply = true
            val wire = MorayWire { method, url, _, body, _ ->
                when {
                    method == "POST" -> {
                        submitted += checkNotNull(body).toString(Charsets.UTF_8)
                        if (failReply) { failReply = false; throw java.io.IOException("response lost") }
                        json(JSONObject().put("schema", "moray.edge-report.v1").put("report", original)
                            .put("receivedAt", "2026-10-30T12:00:01Z").put("verification", "device-reported")
                            .put("reporter", JSONObject().put("kind", "native").put("issuer", profile.issuer)
                                .put("clientId", profile.clientId).put("subject", "original-user")), 201)
                    }
                    url.endsWith("/bundle") -> bundle()
                    else -> json(descriptor())
                }
            }
            val client = MorayPortable(profile, dir, wire, { "source-token" }, "grant1", { clock })
            val saved = client.pull("publication1") { installation() }
            client.retainReport(saved, original)
            reject { client.sendReport("report1") }
            val reopened = MorayPortable(profile, dir, wire, { "rotated-token" }, "grant1", { clock })
            val receipt = reopened.sendReport("report1")
            assertEquals(2, submitted.size)
            assertEquals(submitted[0], submitted[1])
            assertEquals("unknown", receipt.getJSONObject("report").getString("outcome"))
            reject { reopened.retainReport(saved, report().put("outcome", "reported-complete")) }
            reject { MorayPortable(profile, dir, wire, { "other-token" }, "grant2", { clock }).sendReport("report1") }
            assertEquals(2, submitted.size)
        } finally { dir.deleteRecursively() }
    }

    @Test fun unauthorizedAndRedirectsNeverFallbackToAnonymousOrMcp() {
        for (status in listOf(302, 401, 403, 409, 503)) {
            val dir = Files.createTempDirectory("moray-http").toFile()
            try {
                val client = MorayPortable(profile, dir, MorayWire { _, _, access, _, _ ->
                    assertEquals("source-token", access); MorayResponse(status, emptyMap(), byteArrayOf())
                }, { "source-token" }, "grant1", { clock })
                reject { client.descriptor("publication1") }
            } finally { dir.deleteRecursively() }
        }
    }

    @Test fun installedOwnerAndMachineReporterMustMatchBeforeReceiptCustody() {
        val dir = Files.createTempDirectory("moray-machine-owner").toFile()
        try {
            var bundles = 0
            var sends = 0
            var reporterSubject = "other-machine"
            var expectedTenant = "tenant"
            val submitted = mutableListOf<String>()
            val wire = MorayWire { method, url, _, body, _ ->
                when {
                    method == "POST" -> {
                        sends++; submitted += checkNotNull(body).toString(Charsets.UTF_8)
                        json(JSONObject().put("schema", "moray.edge-report.v1").put("report", report())
                            .put("receivedAt", "2026-10-30T12:00:01Z").put("verification", "device-reported")
                            .put("reporter", JSONObject().put("kind", "native").put("issuer", profile.issuer)
                                .put("clientId", profile.clientId).put("subject", reporterSubject)), 201)
                    }
                    url.endsWith("/bundle") -> { bundles++; bundle() }
                    else -> json(descriptor())
                }
            }
            fun client(access: String) = MorayPortable(profile, dir, wire, { access }, "machine-lineage", { clock },
                validateOwner = { owner -> require(owner.sourceString("tenantId") == expectedTenant) },
                validateReporter = { reporter -> require(reporter.sourceString("subject") == "selected-machine") })
            val saved = client("first-access").pull("publication1") { installation() }
            client("first-access").retainReport(saved, report())
            reject { client("first-access").sendReport("report1") }
            assertFalse(java.io.File(dir, "receipt-report1.json").exists())
            reporterSubject = "selected-machine"
            client("renewed-access").sendReport("report1")
            assertEquals(2, sends)
            assertEquals(submitted.first(), submitted.last())
            assertEquals(1, bundles)
            expectedTenant = "other-tenant"
            reject { client("renewed-access").sendReport("report1") }
            reject { client("renewed-access").pull("publication1") { fail("wrong owner installed"); installation() } }
            assertEquals(2, sends)
            assertEquals(1, bundles)
        } finally { dir.deleteRecursively() }
    }
    @Test fun collectionSourceIsClosedAndRetainsUnsupportedPublicationRefusal() {
        val dir = Files.createTempDirectory("collection-refusal").toFile()
        try {
            val d = descriptor()
            val owner = d.getJSONObject("source").getJSONObject("owner")
            val source = JSONObject().put("schemaVersion", "moray.declared-native-collection.v1-proposal")
                .put("origin", "DECLARED").put("owner", owner).put("fixtureId", "collection-fixture")
                .put("versionId", "e".repeat(64)).put("selectionRawSha256", "a".repeat(64))
                .put("selectionRef", JSONObject().put("tenantId", owner.getString("tenantId"))
                    .put("artifactId", "art_" + "b".repeat(32)).put("sha256", "a".repeat(64)))
                .put("profileId", "trollm.native-collection-members.v1-proposal")
                .put("profileSha256", "b".repeat(64)).put("installationSha256", "c".repeat(64))
                .put("qualification", "unqualified")
            d.put("source", source).put("consumerRevision", JSONObject.NULL)
                .put("identity", JSONObject.NULL).put("metadata", JSONObject.NULL)
            d.getJSONObject("availability").put("available", false).put("reason", "NO_EXPORT")
            d.getJSONObject("compatibility").put("status", "unsupported")
            val client = MorayPortable(profile, dir, MorayWire { _, _, _, _, _ -> json(d) },
                { "source-token" }, "grant1", { clock })
            for (memberProfile in listOf("trollm.native-collection-members.v1-proposal",
                "trollm.native-collection-members.v3-proposal")) {
                source.put("profileId", memberProfile)
                try { client.descriptor("publication1"); fail("unsupported collection admitted") }
                catch (failure: MorayFailure) { assertEquals("NO_EXPORT", failure.code) }
            }
            source.put("profileId", "trollm.native-collection-members.v2-proposal")
            reject { client.descriptor("publication1") }
            source.put("profileId", "trollm.native-collection-members.v1-proposal")
            source.put("promptSha256", "a".repeat(64))
            reject { client.descriptor("publication1") }
            source.remove("promptSha256")
            source.getJSONObject("selectionRef").put("sha256", "d".repeat(64))
            reject { client.descriptor("publication1") }
            source.getJSONObject("selectionRef").put("sha256", "a".repeat(64))
            source.put("profileId", "controlled-collection-native-selected.v2-proposal")
            reject { client.descriptor("publication1") }
        } finally { dir.deleteRecursively() }
    }

    @Test fun declaredRefusalIsDecodedWithoutInventingBrowserLineageOrRevision() {
        val dir = Files.createTempDirectory("declared-refusal").toFile()
        try {
            val d = descriptor()
            val owner = d.getJSONObject("source").getJSONObject("owner")
            d.put("source", JSONObject().put("schemaVersion", "moray.declared-native-text.v1")
                .put("origin", "DECLARED").put("owner", owner).put("fixtureId", "text-fixture")
                .put("versionId", "e".repeat(64)).put("fixtureSha256", "a".repeat(64))
                .put("promptSha256", "b".repeat(64)).put("installationSha256", "c".repeat(64))
                .put("qualification", "unqualified"))
            d.put("consumerRevision", JSONObject.NULL).put("identity", JSONObject.NULL).put("metadata", JSONObject.NULL)
            d.getJSONObject("availability").put("available", false).put("reason", "NO_EXPORT")
            d.getJSONObject("compatibility").put("status", "unsupported")
            val client = MorayPortable(profile, dir, MorayWire { _, _, _, _, _ -> json(d) },
                { "source-token" }, "grant1", { clock })
            try { client.descriptor("publication1"); fail("unsupported declared source admitted") }
            catch (failure: MorayFailure) { assertEquals("NO_EXPORT", failure.code) }
            d.getJSONObject("source").put("planSha256", "a".repeat(64))
            reject { client.descriptor("publication1") }
            d.getJSONObject("source").remove("planSha256")
            d.getJSONObject("availability").put("available", true).put("reason", JSONObject.NULL)
            reject { client.descriptor("publication1") }
        } finally { dir.deleteRecursively() }
    }

}
