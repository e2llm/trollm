package com.trollm.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import com.trollm.app.UiError
import com.trollm.app.UiOperation
import com.trollm.app.operationError
import com.trollm.app.combineUiErrors
import com.trollm.app.catchingOperation
import com.trollm.app.FlowSetup
import com.trollm.app.historyEntries
import com.trollm.app.StarterFlows
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.RunSummaryInfo
import uniffi.trollm_uniffi.TrollmRuntime

/** Installed flows from `list_flows` (§9), content-addressed by sha256. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun FlowListScreen(
    runtime: TrollmRuntime,
    runIds: List<String>,
    onOpenRun: (String) -> Unit,
    onRunFlow: (FlowInfo) -> Unit,
    onInstall: () -> Unit,
    onConnections: () -> Unit,
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var flowsLoading by remember { mutableStateOf(true) }
    var flowsLoaded by remember { mutableStateOf(false) }
    var historyLoading by remember(runIds) { mutableStateOf(true) }
    var flows by remember { mutableStateOf<List<FlowInfo>>(emptyList()) }
    var setups by remember { mutableStateOf<Map<String, FlowSetup?>>(emptyMap()) }
    val titles = flows.associate { flow -> flow.sha256 to (setups[flow.sha256]?.title ?: flow.id) }
    var summaries by remember { mutableStateOf<Map<String, RunSummaryInfo>>(emptyMap()) }
    var error by remember { mutableStateOf<UiError?>(null) }
    var historyErrors by remember { mutableStateOf<Map<String, UiError>>(emptyMap()) }
    val history = remember(runIds, summaries, titles) { historyEntries(runIds, summaries, titles) }
    val dateFormat = remember { DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT) }

    androidx.compose.runtime.LaunchedEffect(Unit) {
        val starterError = catchingOperation { StarterFlows.ensureInstalled(context, runtime) }.exceptionOrNull()
        val setupErrors = mutableListOf<UiError>()
        val listingError = catchingOperation {
            val listed = withContext(Dispatchers.IO) {
                runtime.listFlows().map { flow ->
                    val setup = catchingOperation {
                        runtime.flowSetup(flow.sha256)?.let { FlowSetup.parse(it) }
                    }.onFailure { setupErrors += operationError(UiOperation.LOAD_SETUP, it) }.getOrNull()
                    flow to setup
                }
            }
            flowsLoaded = true
            flows = listed.map { it.first }
            setups = listed.associate { it.first.sha256 to it.second }
        }.exceptionOrNull()
        error = combineUiErrors(
            starterError?.let { operationError(UiOperation.INSTALL_STARTERS, it) },
            listingError?.let { operationError(UiOperation.LOAD_FLOWS, it) },
            *setupErrors.toTypedArray(),
        )
        flowsLoading = false
    }

    suspend fun readSummaries(ids: Collection<String>): Map<String, RunSummaryInfo> {
        val failures = mutableMapOf<String, UiError>()
        val result = withContext(Dispatchers.IO) {
            ids.mapNotNull { id ->
                catchingOperation { runtime.runSummary(id) }
                    .onFailure { failure ->
                        val diagnostic = operationError(UiOperation.READ_HISTORY, failure)
                        failures[id] = diagnostic.copy(details = "Run $id\n${diagnostic.details.orEmpty()}")
                    }
                    .getOrNull()?.let { id to it }
            }.toMap()
        }
        historyErrors = ((historyErrors - ids.toSet()) + failures).filterKeys { it in runIds }
        return result
    }

    LaunchedEffect(runtime, runIds, lifecycleOwner) {
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            // A screen return or foreground transition refreshes every saved row.
            summaries = readSummaries(runIds)
            historyLoading = false
            // While a worker remains live, refresh only its rows. The observer
            // stops with the screen and never tails an event log.
            while (true) {
                val activeIds = summaries.filterValues { it.workerActive }.keys
                if (activeIds.isEmpty()) break
                delay(2_000)
                val refreshed = readSummaries(activeIds)
                summaries = (summaries - activeIds) + refreshed
            }
        }
    }

    LazyColumn(
        modifier = Modifier.fillMaxSize().padding(horizontal = 20.dp),
        contentPadding = androidx.compose.foundation.layout.PaddingValues(vertical = 20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item { ScreenHeader("trollm", subtitle = "Choose a flow to get started.") }
        item {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = onInstall, shape = MaterialTheme.shapes.medium) { Text("Install flow") }
                OutlinedButton(onClick = onConnections, shape = MaterialTheme.shapes.medium) { Text("Connections") }
            }
        }
        error?.let { diagnostic -> item { OperationError(diagnostic) } }
        combineUiErrors(*historyErrors.values.toTypedArray())?.let { diagnostic ->
            item { OperationError(diagnostic) }
        }
        item { SectionHeading("Installed flows", if (flowsLoaded) flows.size.toString() else null) }
        if (flowsLoading) {
            item { Text("Loading flows…", modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
        } else if (flowsLoaded && flows.isEmpty()) {
            item { ContentCard { Text("No installed flows. Install a flow bundle to begin.") } }
        } else if (!flowsLoaded) {
            item { Text("Flows could not be loaded.") }
        }
        items(flows) { flow ->
            ContentCard(Modifier.clickable(role = Role.Button, onClickLabel = "Open flow") { onRunFlow(flow) }) {
                Text(titles[flow.sha256] ?: flow.id, style = MaterialTheme.typography.titleMedium)
                setups[flow.sha256]?.description?.takeIf { it.isNotBlank() }?.let { description ->
                    Text(description, style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                Text("Version ${flow.version}", style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        item { SectionHeading("Run history", if (!historyLoading) history.size.toString() else null) }
        if (historyLoading) {
            item { Text("Loading run history…", modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
        } else if (history.isEmpty()) {
            item { ContentCard { Text("No runs yet. Choose a flow to start one.") } }
        }
        items(history, key = { it.id }) { entry ->
            ContentCard(Modifier.clickable(role = Role.Button, onClickLabel = "Open run ${entry.id}") { onOpenRun(entry.id) }) {
                Text(entry.flowTitle, style = MaterialTheme.typography.titleMedium)
                StatusBadge(entry.status, entry.tone)
                Text(entry.startedAtMs?.let { dateFormat.format(Date(it)) } ?: "Time unavailable",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                TechnicalDetails("Run ID: ${entry.id}" + (entry.flowId?.let { "\nFlow ID: $it" } ?: ""))
            }
        }
    }
}
