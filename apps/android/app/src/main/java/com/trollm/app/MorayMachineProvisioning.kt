package com.trollm.app

import org.json.JSONObject
import java.util.UUID

/** Closed installation input. The caller's token/grant/generation cannot be imported. */
object MorayMachineProvisioning {
    fun prepare(input: JSONObject, installationId: String, profile: MorayProfile, previous: JSONObject?): JSONObject {
        require(JsonValues.keys(input) == setOf("schema", "api", "issuer", "clientId", "subject",
            "clientSecret", "installationId", "owner"))
        require(input.sourceString("schema") == "trollm.moray-machine-installation.v1-proposal")
        require(input.sourceString("installationId") == installationId && installationId.isNotBlank())
        require(input.sourceString("api") == profile.api && input.sourceString("issuer") == profile.issuer &&
            input.sourceString("clientId") == profile.clientId)
        require(input.sourceString("subject").matches(Regex("[A-Za-z0-9._:-]{1,256}")))
        require(input.sourceString("clientSecret").length in 16..4096 &&
            input.sourceString("clientSecret").all { it.code in 33..126 })
        val owner = input.getJSONObject("owner")
        require(JsonValues.keys(owner) == setOf("registrationId", "tenantId", "subject", "workerId", "runId", "epoch"))
        for (name in listOf("registrationId", "tenantId", "subject", "workerId", "runId")) {
            require(owner.sourceString(name).matches(Regex("[A-Za-z0-9][A-Za-z0-9_-]{0,127}")))
        }
        require(MorayMachineHttps.integer(owner, "epoch") > 0)
        val samePrincipal = previous != null && previous.sourceString("profile") == profile.key &&
            previous.sourceString("subject") == input.sourceString("subject") &&
            JsonValues.same(previous.getJSONObject("owner"), owner)
        return JSONObject(input.toString())
            .put("profile", profile.key)
            .put("grantId", if (samePrincipal) previous!!.sourceString("grantId") else UUID.randomUUID().toString())
            .put("generation", UUID.randomUUID().toString())
    }
}
