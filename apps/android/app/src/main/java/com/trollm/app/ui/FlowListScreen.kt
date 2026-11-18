package com.trollm.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import com.trollm.app.BundledStarter
import com.trollm.app.FlowSetup
import com.trollm.app.LibraryFlow
import com.trollm.app.RunHistoryEntry
import com.trollm.app.StarterFlows
import com.trollm.app.StatusTone
import com.trollm.app.UiError
import com.trollm.app.UiOperation
import com.trollm.app.catchingOperation
import com.trollm.app.combineUiErrors
import com.trollm.app.featuredLibraryRun
import com.trollm.app.flowSetupOverrideStore
import com.trollm.app.historyEntries
import com.trollm.app.libraryFlows
import com.trollm.app.operationError
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.RunSummaryInfo
import uniffi.trollm_uniffi.TrollmRuntime

/** Installed flows and passive summaries of retained runs. */
@Composable
fun FlowListScreen(
    runtime: TrollmRuntime,
    runIds: List<String>,
    onOpenRun: (String) -> Unit,
    onRunFlow: (FlowInfo) -> Unit,
    onInstall: () -> Unit,
    onConnections: () -> Unit,
    onMoray: () -> Unit = {},
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var flowsLoading by remember { mutableStateOf(true) }
    var flowsLoaded by remember { mutableStateOf(false) }
    var historyLoading by remember(runIds) { mutableStateOf(true) }
    var catalog by remember { mutableStateOf<List<LibraryFlow>>(emptyList()) }
    val titles = remember(catalog) { catalog.associate { entry -> entry.flow.sha256 to entry.title } }
    var summaries by remember { mutableStateOf<Map<String, RunSummaryInfo>>(emptyMap()) }
    var error by remember { mutableStateOf<UiError?>(null) }
    var historyErrors by remember { mutableStateOf<Map<String, UiError>>(emptyMap()) }
    var details by remember { mutableStateOf<LibraryDetails?>(null) }
    val history = remember(runIds, summaries, titles) { historyEntries(runIds, summaries, titles) }
    val currentRun = remember(history, summaries) { featuredLibraryRun(history, summaries) }
    val featuredFlow = catalog.firstOrNull { it.starter == BundledStarter.TEXT_SUMMARY }.takeIf { currentRun == null }
    val otherFlows = catalog.filterNot { it == featuredFlow }
    val pastRuns = history.filterNot { it.id == currentRun?.id }
    val dateFormat = remember { DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT) }

    LaunchedEffect(runtime) {
        val installation = StarterFlows.ensureInstalled(context, runtime)
        val setupErrors = mutableListOf<UiError>()
        val listingError = catchingOperation {
            catalog = withContext(Dispatchers.IO) {
                val overrideStore = flowSetupOverrideStore(context)
                val flows = runtime.listFlows()
                val defaults = mutableMapOf<String, FlowSetup?>()
                val effective = mutableMapOf<String, FlowSetup?>()
                for (flow in flows) {
                    val setup = catchingOperation { runtime.flowSetup(flow.sha256)?.let { FlowSetup.parse(it) } }
                        .onFailure { setupErrors += operationError(UiOperation.LOAD_SETUP, it) }
                        .getOrNull()
                    defaults[flow.sha256] = setup
                    effective[flow.sha256] = setup?.let {
                        // Invalid overrides block preparation, so avoid a stale cost claim.
                        catchingOperation { overrideStore.load(flow.sha256, it) }.getOrNull()
                    }
                }
                libraryFlows(flows, defaults, effective, installation.verified)
            }
            flowsLoaded = true
        }.exceptionOrNull()
        error = combineUiErrors(
            listingError?.let { operationError(UiOperation.LOAD_FLOWS, it) },
            *(installation.errors + setupErrors).toTypedArray(),
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

    Column(Modifier.fillMaxSize().padding(horizontal = 20.dp)) {
        ScreenHeader("Flow library", actions = {
            OverflowMenu("Manage flows") { dismiss ->
                DropdownMenuItem(
                    text = { Text("Import flow file") },
                    onClick = {
                        dismiss()
                        onInstall()
                    },
                    modifier = Modifier.heightIn(min = 48.dp),
                )
                DropdownMenuItem(
                    text = { Text("Connections") },
                    onClick = {
                        dismiss()
                        onConnections()
                    },
                    modifier = Modifier.heightIn(min = 48.dp),
                )
                DropdownMenuItem(
                    text = { Text("Moray source") },
                    onClick = {
                        dismiss()
                        onMoray()
                    },
                    modifier = Modifier.heightIn(min = 48.dp),
                )
            }
        })
        LazyColumn(
            modifier = Modifier.weight(1f),
            contentPadding = PaddingValues(top = 12.dp, bottom = 20.dp),
        ) {
            currentRun?.let { entry ->
                item(key = "featured-run") {
                    val cardTone = entry.tone
                    val colors = MaterialTheme.colorScheme
                    FeaturedCard(
                        title = entry.flowTitle,
                        action = "View run",
                        onOpen = { onOpenRun(entry.id) },
                        onDetails = { details = entry.details(onOpenRun) },
                        background = if (cardTone == StatusTone.WARNING) colors.secondaryContainer else colors.primaryContainer,
                        foreground = if (cardTone == StatusTone.WARNING) colors.onSecondaryContainer else colors.onPrimaryContainer,
                    ) {
                        StatusBadge(entry.status.removeSuffix(" · Resume"), cardTone)
                        HistoryTime(entry, dateFormat)
                    }
                }
            }
            error?.let { diagnostic -> item { OperationError(diagnostic) } }
            featuredFlow?.let { entry ->
                item(key = "featured-flow") {
                    FeaturedCard(
                        title = entry.title,
                        action = "Open flow",
                        onOpen = { onRunFlow(entry.flow) },
                        onDetails = { details = entry.details(onRunFlow) },
                    ) { FlowFacts(entry) }
                }
            }
            if (featuredFlow == null || otherFlows.isNotEmpty()) {
                item {
                    LibrarySection(
                        if (featuredFlow == null) "Installed flows" else "Other installed flows",
                        spaced = currentRun != null || featuredFlow != null,
                    )
                }
            }
            if (flowsLoading) {
                item { LibraryMessage("Loading flows…", announce = true) }
            } else if (flowsLoaded && catalog.isEmpty()) {
                item { LibraryMessage("No installed flows. Use Manage flows to import a flow file.") }
            } else if (!flowsLoaded) {
                item { LibraryMessage("Flows could not be loaded.") }
            }
            items(otherFlows, key = { "flow:${it.flow.sha256}" }) { entry ->
                LibraryRow(
                    title = entry.title,
                    onOpenLabel = "Open flow",
                    onOpen = { onRunFlow(entry.flow) },
                    onDetails = { details = entry.details(onRunFlow) },
                ) { FlowFacts(entry) }
            }
            item { LibrarySection("Run history", spaced = true) }
            combineUiErrors(*historyErrors.values.toTypedArray())?.let { diagnostic ->
                item { OperationError(diagnostic) }
            }
            if (historyLoading) {
                item { LibraryMessage("Loading run history…", announce = true) }
            } else if (history.isEmpty()) {
                item { LibraryMessage("Your runs appear here after you start a flow.") }
            } else if (pastRuns.isEmpty()) {
                item { LibraryMessage("Your current run is shown above.") }
            }
            items(pastRuns, key = { "run:${it.id}" }) { entry ->
                LibraryRow(
                    title = entry.flowTitle,
                    onOpenLabel = "View run",
                    onOpen = { onOpenRun(entry.id) },
                    onDetails = { details = entry.details(onOpenRun) },
                ) {
                    HistoryTime(entry, dateFormat)
                    StatusBadge(entry.status.removeSuffix(" · Resume"), entry.tone)
                }
            }
        }
    }
    details?.let { selected ->
        AlertDialog(
            onDismissRequest = { details = null },
            title = { Text(selected.title) },
            text = {
                SelectionContainer {
                    Text(
                        selected.metadata,
                        modifier = Modifier.verticalScroll(rememberScrollState()),
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            },
            confirmButton = {
                Button(
                    onClick = {
                        details = null
                        selected.onOpen()
                    },
                    modifier = Modifier.heightIn(min = 48.dp),
                ) { Text(selected.openLabel) }
            },
            dismissButton = {
                TextButton(onClick = { details = null }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Done") }
            },
        )
    }
}

private data class LibraryDetails(
    val title: String,
    val metadata: String,
    val openLabel: String,
    val onOpen: () -> Unit,
)

private fun LibraryFlow.details(onOpen: (FlowInfo) -> Unit): LibraryDetails {
    val selected = flow.copy()
    return LibraryDetails(
        title = title,
        metadata = "Version ${selected.version}\nFlow ID: ${selected.id}\nFlow SHA-256: ${selected.sha256}",
        openLabel = "Open flow",
        onOpen = { onOpen(selected) },
    )
}

private fun RunHistoryEntry.details(onOpen: (String) -> Unit) = LibraryDetails(
    title = flowTitle,
    metadata = "Run ID: $id" + (flowId?.let { "\nFlow ID: $it" } ?: ""),
    openLabel = "View run",
    onOpen = { onOpen(id) },
)

@Composable
private fun LibrarySection(title: String, spaced: Boolean) {
    Column(Modifier.padding(top = if (spaced) 24.dp else 0.dp, bottom = 4.dp)) {
        SectionHeading(title)
    }
}

@Composable
private fun LibraryMessage(message: String, announce: Boolean = false) {
    Text(
        message,
        modifier = Modifier.padding(vertical = 8.dp).semantics {
            if (announce) liveRegion = LiveRegionMode.Polite
        },
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun FeaturedCard(
    title: String,
    action: String,
    onOpen: () -> Unit,
    onDetails: () -> Unit,
    background: Color = MaterialTheme.colorScheme.primaryContainer,
    foreground: Color = MaterialTheme.colorScheme.onPrimaryContainer,
    content: @Composable () -> Unit,
) {
    Surface(color = background, contentColor = foreground, shape = MaterialTheme.shapes.large) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(title, modifier = Modifier.weight(1f).semantics { heading() }, style = MaterialTheme.typography.titleLarge)
                DetailsButton("Details for $title", onDetails)
            }
            content()
            Button(onClick = onOpen, modifier = Modifier.heightIn(min = 48.dp), shape = MaterialTheme.shapes.medium) {
                Text(action)
            }
        }
    }
}

@Composable
private fun LibraryRow(
    title: String,
    onOpenLabel: String,
    onOpen: () -> Unit,
    onDetails: () -> Unit,
    content: @Composable () -> Unit,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(
            Modifier.weight(1f).heightIn(min = 48.dp)
                .clickable(role = Role.Button, onClickLabel = onOpenLabel, onClick = onOpen)
                .padding(vertical = 12.dp, horizontal = 4.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            content()
        }
        DetailsButton("Details for $title", onDetails)
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}

@Composable
private fun FlowFacts(entry: LibraryFlow) {
    listOfNotNull(entry.description, entry.modelRequirement, entry.connectionRequirement).forEach { fact ->
        Text(fact, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun HistoryTime(entry: RunHistoryEntry, dateFormat: DateFormat) {
    val time = entry.startedAtMs?.let { "Started ${dateFormat.format(Date(it))}" } ?: "Start time unavailable"
    Text(time, style = MaterialTheme.typography.bodyMedium)
}
