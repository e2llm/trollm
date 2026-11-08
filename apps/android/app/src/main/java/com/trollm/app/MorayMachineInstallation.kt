package com.trollm.app

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.attribute.PosixFilePermission
import java.util.UUID

/** Operator-private installation import, never an exported intent extra or APK asset. */
object MorayMachineInstallation {
    private val providers = mutableMapOf<String, MorayMachineTokens>()

    fun deviceId(context: Context): String = synchronized(this) {
        val prefs = context.getSharedPreferences("moray-source-device", 0)
        prefs.getString("id", null) ?: UUID.randomUUID().toString().also {
            check(prefs.edit().putString("id", it).commit())
        }
    }

    fun importPrivate(context: Context): MorayProfile {
        try {
            val store = SecretsStore(context, source = true)
            // Anchor before private input I/O; removal while reading cannot be adopted.
            val originalRevision = store.machineRevision()
            val file = File(context.filesDir, "trollm/moray-machine-provisioning.json")
            val path = file.toPath()
            require(Files.isRegularFile(path, LinkOption.NOFOLLOW_LINKS) && !Files.isSymbolicLink(path))
            require(Files.getOwner(path) == Files.getOwner(context.filesDir.toPath()))
            require(Files.getPosixFilePermissions(path).all { it in setOf(
                PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE) })
            val value = file.inputStream().use { MorayMachineHttps.json(BundleInput.read(it, 16_384)) }
            val profile = MorayProfile(value.sourceString("api"), value.sourceString("issuer"),
                value.sourceString("clientId"), BuildConfig.SOURCE_REVISION)
            // A failed read of existing custody must never manufacture a new lineage.
            val original = store.machineState(profile.key)
            if (original.revision != originalRevision) throw MorayFailure("SOURCE_MACHINE_CREDENTIAL_CHANGED")
            val prepared = MorayMachineProvisioning.prepare(value, deviceId(context), profile, original.value)
            // Consume the one-time private input before enabling it. A crash here is
            // unprovisioned, not a token/credential replay on the next app open.
            Files.delete(path)
            store.replaceMachine(profile.key, originalRevision, prepared)
            return profile
        } catch (safe: MorayFailure) { throw safe
        } catch (_: Exception) { throw MorayFailure("SOURCE_MACHINE_PROVISIONING_INVALID") }
    }

    fun remove(context: Context, profile: MorayProfile) {
        SecretsStore(context, source = true).removeMachine(profile.key)
    }

    fun provider(context: Context, profile: MorayProfile): MorayMachineTokens {
        val store = SecretsStore(context, source = true)
        fun selected(): MorayMachineCredential? = store.machine(profile.key)?.let { value ->
            require(value.sourceString("installationId") == deviceId(context))
            require(value.sourceString("api") == profile.api && value.sourceString("issuer") == profile.issuer &&
                value.sourceString("clientId") == profile.clientId)
            MorayMachineCredential(profile.key, value.sourceString("grantId"),
                profile.issuer + "/protocol/openid-connect/token", profile.clientId,
                value.sourceString("subject"), value.sourceString("clientSecret"), value.sourceString("generation"))
        }
        val generation = selected()?.generation ?: throw MorayFailure("SOURCE_MACHINE_PROVISIONING_REQUIRED")
        return synchronized(providers) {
            val key = profile.key + ":" + generation
            providers[key] ?: MorayMachineTokens(profile, ::selected, MorayMachineHttps(context.filesDir)).also {
                // Original clients keep their own provider and remain generation-fenced.
                providers.clear()
                providers[key] = it
            }
        }
    }

    fun requireOwner(context: Context, profile: MorayProfile, owner: JSONObject) {
        val selected = SecretsStore(context, source = true).machine(profile.key)
            ?: throw MorayFailure("SOURCE_MACHINE_PROVISIONING_REQUIRED")
        if (!JsonValues.same(selected.getJSONObject("owner"), owner)) throw MorayFailure("SOURCE_MACHINE_OWNER_MISMATCH")
    }

    fun requireReporter(context: Context, profile: MorayProfile, reporter: JSONObject) {
        val selected = SecretsStore(context, source = true).machine(profile.key)
            ?: throw MorayFailure("SOURCE_MACHINE_PROVISIONING_REQUIRED")
        if (reporter.sourceString("subject") != selected.sourceString("subject")) {
            throw MorayFailure("SOURCE_MACHINE_REPORTER_MISMATCH")
        }
    }
}
