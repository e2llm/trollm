package com.trollm.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
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
    var flows by remember { mutableStateOf<List<FlowInfo>>(emptyList()) }
    var titles by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
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
                    val title = catchingOperation {
                        runtime.flowSetup(flow.sha256)?.let { FlowSetup.parse(it).title }
                    }.onFailure { setupErrors += operationError(UiOperation.LOAD_SETUP, it) }.getOrNull() ?: flow.id
                    flow to title
                }
            }
            flows = listed.map { it.first }
            titles = listed.associate { it.first.sha256 to it.second }
        }.exceptionOrNull()
        error = combineUiErrors(
            starterError?.let { operationError(UiOperation.INSTALL_STARTERS, it) },
            listingError?.let { operationError(UiOperation.LOAD_FLOWS, it) },
            *setupErrors.toTypedArray(),
        )
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

    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text("trollm", style = MaterialTheme.typography.headlineSmall)
            Button(onClick = onInstall) { Text("Install bundle") }
        }
        Button(onClick = onConnections) { Text("Connections") }
        LazyColumn(Modifier.fillMaxSize().padding(top = 16.dp)) {
            error?.let { diagnostic -> item { OperationError(diagnostic) } }
            combineUiErrors(*historyErrors.values.toTypedArray())?.let { diagnostic ->
                item { OperationError(diagnostic) }
            }
            item { Text("Installed flows", style = MaterialTheme.typography.titleMedium) }
            items(flows) { flow ->
                Card(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .clickable { onRunFlow(flow) },
                ) {
                    Column(Modifier.padding(12.dp)) {
                        Text(titles[flow.sha256] ?: flow.id, style = MaterialTheme.typography.titleMedium)
                        Text(
                            "v${flow.version} · sha256 ${flow.sha256.take(12)}…",
                            fontFamily = FontFamily.Monospace,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
            if (history.isNotEmpty()) {
                item { Text("Run history", style = MaterialTheme.typography.titleMedium) }
                items(history, key = { it.id }) { entry ->
                    Card(Modifier.fillMaxWidth().padding(vertical = 4.dp).clickable { onOpenRun(entry.id) }) {
                        Column(Modifier.padding(12.dp)) {
                            Text(entry.flowTitle, style = MaterialTheme.typography.titleMedium)
                            entry.flowId?.takeIf { it != entry.flowTitle }?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                            Text(
                                "${entry.startedAtMs?.let { dateFormat.format(Date(it)) } ?: "Time unavailable"} · ${entry.status}",
                                style = MaterialTheme.typography.bodyMedium,
                            )
                            Text(entry.id, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        }
    }
}
