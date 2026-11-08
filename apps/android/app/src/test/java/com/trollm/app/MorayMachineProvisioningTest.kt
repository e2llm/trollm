package com.trollm.app

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class MorayMachineProvisioningTest {
    private val profile = MorayProfile("https://source.test/v1/portable-publications",
        "https://issuer.test/realms/source", "installed-client", "a".repeat(40))
    private fun input() = JSONObject().put("schema", "trollm.moray-machine-installation.v1-proposal")
        .put("api", profile.api).put("issuer", profile.issuer).put("clientId", profile.clientId)
        .put("subject", "declared-service-subject").put("clientSecret", "declared-private-test-secret")
        .put("installationId", "declared-device")
        .put("owner", JSONObject().put("registrationId", "registration").put("tenantId", "tenant")
            .put("subject", "owner").put("workerId", "worker").put("runId", "run").put("epoch", 1))
    private fun prepare(value: JSONObject = input(), previous: JSONObject? = null) =
        MorayMachineProvisioning.prepare(value, "declared-device", profile, previous)

    @Test fun secretRotationKeepsLineageAndChangesStorageGeneration() {
        val originalInput = input()
        val bytes = originalInput.toString()
        val first = prepare(originalInput)
        val rotated = prepare(input().put("clientSecret", "declared-rotated-test-secret"), first)
        assertEquals(first.sourceString("grantId"), rotated.sourceString("grantId"))
        assertNotEquals(first.sourceString("generation"), rotated.sourceString("generation"))
        assertEquals(bytes, originalInput.toString())
        for (changed in listOf(input().put("subject", "other-principal"), input().apply {
            getJSONObject("owner").put("epoch", 2)
        })) assertNotEquals(first.sourceString("grantId"), prepare(changed, first).sourceString("grantId"))
    }

    @Test fun wrongDeviceAndUploadedAuthorityCannotSelectMachineLineage() {
        val variants = listOf(input().put("installationId", "other-device"), input().put("grantId", "uploaded"),
            input().put("accessToken", "uploaded"), input().put("generation", "uploaded"),
            input().put("clientId", "other-client"), input().put("issuer", "https://other.test/"),
            input().put("clientSecret", "short"), input().put("subject", 42), input().apply {
                getJSONObject("owner").put("epoch", "1")
            })
        for (value in variants) {
            try { prepare(value); fail("accepted invalid private provisioning") }
            catch (_: IllegalArgumentException) {}
        }
    }
}
