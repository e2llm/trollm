package com.trollm.app

import java.util.UUID

/** Pure identity/ownership rules shared by storage, navigation and the service. */
object RunIdentity {
    fun valid(id: String?): Boolean = id != null && runCatching {
        UUID.fromString(id).toString() == id
    }.getOrDefault(false)

    fun discover(saved: Collection<String>, directories: Collection<String>): List<String> =
        (saved + directories).filter(::valid).distinct().sorted()
}

class ActiveRuns {
    private val ids = linkedSetOf<String>()
    fun attach(id: String): Boolean {
        require(RunIdentity.valid(id))
        return ids.add(id)
    }
    fun complete(id: String): Boolean {
        ids.remove(id)
        return ids.isEmpty()
    }
    fun first(): String? = ids.firstOrNull()
    fun isEmpty(): Boolean = ids.isEmpty()
}
