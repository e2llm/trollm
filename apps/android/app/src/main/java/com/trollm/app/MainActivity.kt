package com.trollm.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.listSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.trollm.app.ui.ConnectionsScreen
import com.trollm.app.ui.FlowListScreen
import com.trollm.app.ui.InstallBundleScreen
import com.trollm.app.ui.MoraySourceScreen
import com.trollm.app.ui.RunScreen
import com.trollm.app.ui.TrollmTheme
import java.util.UUID
import uniffi.trollm_uniffi.FlowInfo

private val NavigationSaver = listSaver<AppNavigation, Any?>(
    save = { it.savedValues() }, restore = { AppNavigation.restore(it) },
)

class MainActivity : ComponentActivity() {
    private var notificationRun by mutableStateOf<String?>(null)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        SelectedNativeLauncher.accept(this, intent)
        notificationRun = intent.getStringExtra(RunService.EXTRA_RUN_ID)?.takeIf(RunIdentity::valid)
        intent.removeExtra(RunService.EXTRA_RUN_ID)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        SelectedNativeLauncher.accept(this, intent)
        enableEdgeToEdge()
        val runtime = RuntimeHolder.get(this)
        val history = RunHistory(this)
        val runStarts = ViewModelProvider(this)[RunStartViewModel::class.java]
        notificationRun = intent.getStringExtra(RunService.EXTRA_RUN_ID)?.takeIf(RunIdentity::valid)
        intent.removeExtra(RunService.EXTRA_RUN_ID)
        val selectedAtCreation = notificationRun ?: history.selected()
        setContent {
            TrollmTheme {
                val keyboard = LocalSoftwareKeyboardController.current
                val runStates = rememberSaveableStateHolder()
                val startRecord by runStarts.record.collectAsStateWithLifecycle()
                var navigation by rememberSaveable(stateSaver = NavigationSaver) {
                    mutableStateOf(selectedAtCreation?.let { AppNavigation.savedRun(it) }
                        ?: AppNavigation())
                }
                fun navigate(next: AppNavigation) {
                    keyboard?.hide()
                    val previousKey = navigation.runScreenKey
                    if (previousKey != null && previousKey != next.runScreenKey) {
                        runStates.removeState(previousKey)
                    }
                    navigation = next
                    if (next.page == AppPage.LIBRARY) history.select(null)
                    else next.runId?.let(history::select)
                }
                val back = { navigate(navigation.back()) }
                fun openFlow(flow: FlowInfo, installation: String? = null, association: String? = null) {
                    navigate(AppNavigation.flow(flow, "flow:${UUID.randomUUID()}", installation, association))
                }
                LaunchedEffect(Unit) {
                    val restored = navigation.reconcileSelectedRun(selectedAtCreation)
                    if (restored != navigation) navigate(restored)
                }
                LaunchedEffect(notificationRun) {
                    notificationRun?.let {
                        navigate(AppNavigation.savedRun(it))
                        notificationRun = null
                    }
                }
                LaunchedEffect(navigation.runId, startRecord) {
                    val record = startRecord
                    val started = record?.phase as? RunStartPhase.Started
                    if (record != null && started?.result?.runId == navigation.runId && navigation.runId != null) {
                        runStarts.acknowledge(record.screenKey, checkNotNull(navigation.runId))
                    }
                }
                BackHandler(navigation.page != AppPage.LIBRARY) { back() }
                when (navigation.page) {
                    AppPage.MORAY -> MoraySourceScreen(runtime, onOpen = { saved ->
                        val installed = saved.getJSONObject("installation")
                        openFlow(
                            FlowInfo(sha256 = installed.getString("flowSha256"),
                                id = installed.getString("flowId"), version = installed.getLong("flowVersion").toUInt()),
                            installed.getString("installationSha256"), saved.toString(),
                        )
                    }, onBack = back)
                    AppPage.CONNECTIONS -> ConnectionsScreen(
                        onBack = back,
                        initialServer = navigation.connectionServer,
                        initialEndpoint = navigation.connectionEndpoint,
                    )
                    AppPage.IMPORT -> InstallBundleScreen(runtime = runtime, onDone = back,
                        onOpen = { openFlow(it) })
                    AppPage.FLOW -> {
                        val screen = navigation
                        val key = checkNotNull(screen.runScreenKey)
                        runStates.SaveableStateProvider(key) {
                            RunScreen(
                                runtime = runtime,
                                flow = screen.flow,
                                existingRunId = screen.runId,
                                runScreenKey = key,
                                runStartViewModel = runStarts,
                                sourceInstallation = screen.sourceInstallation,
                                sourceAssociation = screen.sourceAssociation,
                                onRunStarted = { id ->
                                    // Keep the composition key and startup diagnostics. A
                                    // later notification must not be replaced by an old callback.
                                    if (navigation.runScreenKey == key && navigation.page == AppPage.FLOW) {
                                        navigate(navigation.started(id))
                                        runStarts.acknowledge(key, id)
                                    }
                                },
                                onBack = back,
                                onOpenConnection = { server, endpoint ->
                                    navigate(navigation.connections(server, endpoint))
                                },
                            )
                        }
                    }
                    AppPage.LIBRARY -> FlowListScreen(
                        runtime = runtime,
                        runIds = history.ids(),
                        onOpenRun = { navigate(AppNavigation.savedRun(it)) },
                        onRunFlow = { openFlow(it) },
                        onInstall = { navigate(AppNavigation(page = AppPage.IMPORT)) },
                        onMoray = { navigate(AppNavigation(page = AppPage.MORAY)) },
                        onConnections = { navigate(navigation.connections()) },
                    )
                }
            }
        }
    }
}
