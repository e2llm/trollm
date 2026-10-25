package com.trollm.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.rememberSaveable
import com.trollm.app.ui.FlowListScreen
import com.trollm.app.ui.ConnectionsScreen
import com.trollm.app.ui.InstallBundleScreen
import com.trollm.app.ui.RunScreen
import com.trollm.app.ui.TrollmTheme
import uniffi.trollm_uniffi.FlowInfo

class MainActivity : ComponentActivity() {
    private var notificationRun by mutableStateOf<String?>(null)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        notificationRun = intent.getStringExtra(RunService.EXTRA_RUN_ID)?.takeIf(RunIdentity::valid)
        intent.removeExtra(RunService.EXTRA_RUN_ID)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val runtime = RuntimeHolder.get(this)
        val history = RunHistory(this)
        notificationRun = intent.getStringExtra(RunService.EXTRA_RUN_ID)?.takeIf(RunIdentity::valid)
        intent.removeExtra(RunService.EXTRA_RUN_ID)
        setContent {
            TrollmTheme {
                var runId by rememberSaveable { mutableStateOf(notificationRun ?: history.selected()) }
                var flowId by rememberSaveable { mutableStateOf<String?>(null) }
                var flowSha by rememberSaveable { mutableStateOf<String?>(null) }
                var flowVersion by rememberSaveable { mutableStateOf(0L) }
                var installing by rememberSaveable { mutableStateOf(false) }
                var connections by rememberSaveable { mutableStateOf(false) }
                androidx.compose.runtime.LaunchedEffect(notificationRun) {
                    notificationRun?.let {
                        runId = it
                        history.select(it)
                        installing = false
                        connections = false
                    }
                }
                val back = {
                    runId = null
                    flowId = null
                    flowSha = null
                    installing = false
                    connections = false
                    history.select(null)
                    notificationRun = null
                }
                androidx.activity.compose.BackHandler(runId != null || flowSha != null || installing || connections) { back() }
                when {
                    connections -> ConnectionsScreen(onBack = { back() })
                    installing -> InstallBundleScreen(runtime = runtime, onDone = { back() })
                    runId != null || flowSha != null -> RunScreen(
                        runtime = runtime,
                        flow = flowSha?.let { FlowInfo(sha256 = it, id = flowId ?: "Flow", version = flowVersion.toUInt()) },
                        existingRunId = runId,
                        onRunStarted = { runId = it },
                        onBack = { back() },
                    )
                    else -> FlowListScreen(
                        runtime = runtime,
                        runIds = history.ids(),
                        onOpenRun = { runId = it; history.select(it) },
                        onRunFlow = { flowId = it.id; flowSha = it.sha256; flowVersion = it.version.toLong() },
                        onInstall = { installing = true },
                        onConnections = { connections = true },
                    )
                }
            }
        }
    }
}
