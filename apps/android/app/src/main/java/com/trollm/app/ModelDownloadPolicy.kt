package com.trollm.app

/** Consent belongs to the current preparation screen, not a global preference. */
data class ModelDownloadPolicy(val allowMetered: Boolean = false) {
    val allowRoaming: Boolean = false
    val description: String
        get() = if (allowMetered) "Metered data allowed; roaming blocked."
            else "Wi-Fi or another unmetered network required; roaming blocked."

    fun waitingMessage(): String = if (allowMetered) "Waiting for a network connection; roaming is blocked."
        else "Waiting for Wi-Fi or another unmetered network."
}

/** A batch is an explicit snapshot of missing assets; shared weights transfer once. */
fun remainingModelDownloads(models: Map<String, ModelRequirement>, readySlots: Set<String>): Map<String, ModelRequirement> {
    val readyHashes = readySlots.mapNotNull { models[it]?.sha256 }.toSet()
    val seen = readyHashes.toMutableSet()
    return models.filter { (_, model) -> seen.add(model.sha256) }
}
