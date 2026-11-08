package com.trollm.app

import org.json.JSONObject
import java.util.UUID

/** Durable selection revision is independent of the stable authorization lineage. */
class MorayMachineState(val revision: String?, val value: JSONObject?) {
    override fun toString(): String = "MorayMachineState(redacted)"
}

class MorayMachineGrants(
    private val read: () -> MorayMachineState,
    private val write: (MorayMachineState) -> Unit,
    private val lock: Any,
) {
    fun revision(): String? = synchronized(lock) { read().revision }

    fun state(profileKey: String): MorayMachineState = synchronized(lock) {
        require(profileKey.matches(Regex("[a-f0-9]{64}")))
        val current = read()
        MorayMachineState(current.revision,
            current.value?.takeIf { it.sourceString("profile") == profileKey })
    }

    fun replace(profileKey: String, expectedRevision: String?, value: JSONObject) = synchronized(lock) {
        require(profileKey.matches(Regex("[a-f0-9]{64}")) && value.sourceString("profile") == profileKey)
        if (read().revision != expectedRevision) throw MorayFailure("SOURCE_MACHINE_CREDENTIAL_CHANGED")
        write(MorayMachineState(UUID.randomUUID().toString(), value))
    }

    fun remove(profileKey: String) = synchronized(lock) {
        require(profileKey.matches(Regex("[a-f0-9]{64}")))
        val current = read()
        if (current.value != null && current.value.sourceString("profile") != profileKey) {
            throw MorayFailure("SOURCE_MACHINE_PROFILE_MISMATCH")
        }
        // Even absent removal has a durable revision: no pending import can revive it.
        write(MorayMachineState(UUID.randomUUID().toString(), null))
    }
}
