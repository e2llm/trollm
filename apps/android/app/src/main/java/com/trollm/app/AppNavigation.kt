package com.trollm.app

import uniffi.trollm_uniffi.FlowInfo

enum class AppPage { LIBRARY, FLOW, CONNECTIONS, IMPORT, MORAY }

/** Navigation is passive. A flow keeps its UI state while visiting Connections. */
data class AppNavigation(
    val page: AppPage = AppPage.LIBRARY,
    val flow: FlowInfo? = null,
    val runId: String? = null,
    val runScreenKey: String? = null,
    val sourceInstallation: String? = null,
    val sourceAssociation: String? = null,
    val connectionServer: String? = null,
    val connectionEndpoint: String? = null,
    val returnToFlow: Boolean = false,
) {
    fun connections(server: String? = null, endpoint: String? = null): AppNavigation =
        if (page == AppPage.FLOW) copy(
            page = AppPage.CONNECTIONS, connectionServer = server,
            connectionEndpoint = endpoint, returnToFlow = true,
        ) else AppNavigation(
            page = AppPage.CONNECTIONS, connectionServer = server, connectionEndpoint = endpoint,
        )

    fun back(): AppNavigation = if (page == AppPage.CONNECTIONS && returnToFlow) {
        copy(page = AppPage.FLOW, connectionServer = null, connectionEndpoint = null, returnToFlow = false)
    } else AppNavigation()

    fun started(id: String): AppNavigation {
        require(page == AppPage.FLOW && flow != null && runScreenKey != null)
        require(RunIdentity.valid(id))
        return copy(runId = id)
    }

    /** A genuine selection recorded after an older draft snapshot takes precedence. */
    fun reconcileSelectedRun(id: String?): AppNavigation =
        if (id != null && RunIdentity.valid(id) && runId == null &&
            (page == AppPage.FLOW || (page == AppPage.CONNECTIONS && returnToFlow))) {
            savedRun(id)
        } else this

    /** Values supported by Android's saved-instance-state registry; no credentials. */
    fun savedValues(): List<Any?> = listOf(
        page.name, flow?.sha256, flow?.id, flow?.version?.toLong(), runId, runScreenKey,
        sourceInstallation, sourceAssociation, connectionServer, connectionEndpoint, returnToFlow,
    )

    companion object {
        fun savedRun(id: String): AppNavigation {
            require(RunIdentity.valid(id))
            return AppNavigation(page = AppPage.FLOW, runId = id, runScreenKey = "run:$id")
        }

        fun flow(
            info: FlowInfo, key: String, installation: String? = null, association: String? = null,
        ): AppNavigation = AppNavigation(
            page = AppPage.FLOW, flow = info, runScreenKey = key,
            sourceInstallation = installation, sourceAssociation = association,
        )

        fun restore(values: List<Any?>): AppNavigation = runCatching {
            require(values.size == 11)
            val page = AppPage.valueOf(values[0] as String)
            val sha = values[1] as String?
            val flow = sha?.let {
                val version = values[3] as Long
                require(version in 0..UInt.MAX_VALUE.toLong())
                FlowInfo(sha256 = it, id = values[2] as String, version = version.toUInt())
            }
            val runId = values[4] as String?
            require(runId == null || RunIdentity.valid(runId))
            val key = values[5] as String?
            val returning = values[10] as Boolean
            if (page == AppPage.FLOW || returning) {
                require(!key.isNullOrBlank() && (flow != null || runId != null))
            }
            require(!returning || page == AppPage.CONNECTIONS)
            AppNavigation(page, flow, runId, key, values[6] as String?, values[7] as String?,
                values[8] as String?, values[9] as String?, returning)
        }.getOrDefault(AppNavigation())
    }
}
