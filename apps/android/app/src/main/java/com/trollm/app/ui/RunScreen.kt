package com.trollm.app.ui

import android.Manifest
import android.content.Context
import android.content.ClipData
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.Lifecycle
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.mapSaver
import kotlinx.coroutines.launch
import com.trollm.app.UiError
import com.trollm.app.UiOperation
import com.trollm.app.operationError
import com.trollm.app.combineUiErrors
import com.trollm.app.catchingOperation
import com.trollm.app.RunHistory
import com.trollm.app.FlowSetup
import com.trollm.app.StructuredInput
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import com.trollm.app.RunService
import com.trollm.app.RunPresentation
import com.trollm.app.presentRun
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.TrollmRuntime

/**
 * Run screen: user input → `start_run` → tail the event log via
 * `tail_events` polling. Progress lines come from event types
 * ([renderEvent]); current worker activity supplies the live status label.
 * The foreground service holds the wake lock during active execution.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RunScreen(
    runtime: TrollmRuntime,
    flow: FlowInfo?,
    existingRunId: String?,
    onRunStarted: (String) -> Unit,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboard.current
    var userInput by rememberSaveable(flow?.sha256) { mutableStateOf("") }
    val valuesSaver = remember { mapSaver<Map<String, String>>(
        save = { it }, restore = { saved -> saved.mapValues { it.value as? String ?: "" } },
    ) }
    var structuredValues by rememberSaveable(flow?.sha256, stateSaver = valuesSaver) { mutableStateOf(emptyMap<String, String>()) }
    var inputAttempted by remember(flow?.sha256) { mutableStateOf(false) }
    val runId = existingRunId
    val scope = rememberCoroutineScope()
    var starting by remember(flow?.sha256) { mutableStateOf(false) }
    var setup by remember(flow?.sha256) { mutableStateOf<FlowSetup?>(null) }
    var setupChecked by remember(flow?.sha256) { mutableStateOf(false) }
    var preparing by rememberSaveable(flow?.sha256) { mutableStateOf(false) }
    var configurationJson by rememberSaveable(flow?.sha256) { mutableStateOf<String?>(null) }
    var finalReport by remember(runId) { mutableStateOf<String?>(null) }
    var lines by remember(runId) { mutableStateOf<List<String>>(emptyList()) }
    var error by remember(runId, flow?.sha256) { mutableStateOf<UiError?>(null) }
    // Keep post-start warnings when the successful UUID changes the screen state.
    var startedRunError by remember(flow?.sha256) { mutableStateOf<UiError?>(null) }
    var pollingError by remember(runId) { mutableStateOf<UiError?>(null) }
    var showTechnicalEvents by remember(runId) { mutableStateOf(false) }
    var presentation by remember(runId) { mutableStateOf<RunPresentation?>(null) }
    var confirmCancel by remember(runId) { mutableStateOf(false) }
    var requesting by remember(runId) { mutableStateOf(false) }
    var pollingGeneration by remember(runId) { mutableStateOf(0) }
    val notificationPreferences = remember(context) { context.getSharedPreferences("run-notifications", Context.MODE_PRIVATE) }
    var notificationsEnabled by remember { mutableStateOf(NotificationManagerCompat.from(context).areNotificationsEnabled()) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        notificationsEnabled = NotificationManagerCompat.from(context).areNotificationsEnabled()
    }
    val permissionLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        notificationsEnabled = NotificationManagerCompat.from(context).areNotificationsEnabled()
    }
    val prepareNotifications = {
        notificationsEnabled = NotificationManagerCompat.from(context).areNotificationsEnabled()
        if (!notificationsEnabled && !notificationPreferences.getBoolean("requested", false)) {
            notificationPreferences.edit().putBoolean("requested", true).apply()
            permissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    val listState = rememberLazyListState()
    val inputScroll = rememberScrollState()

    LaunchedEffect(flow?.sha256) {
        val selected = flow ?: return@LaunchedEffect
        catchingOperation {
            withContext(Dispatchers.IO) { runtime.flowSetup(selected.sha256)?.let { FlowSetup.parse(it) } }
        }.onSuccess {
            setup = it
            setupChecked = true
        }.onFailure { error = operationError(UiOperation.LOAD_SETUP, it) }
    }

    if (preparing && flow != null && runId == null) {
        FlowSetupScreen(
            runtime = runtime,
            flowSha = flow.sha256,
            onReady = { configurationJson = it; preparing = false },
            onBack = { preparing = false },
        )
        return
    }

    // Tail loop: incremental cursor over events.jsonl; status via poll_status.
    LaunchedEffect(runId, pollingGeneration) {
        val id = runId ?: return@LaunchedEffect
        var cursor = 0uL
        lines = emptyList()
        while (isActive) {
            val step = catchingOperation {
                withContext(Dispatchers.IO) {
                    val events = runtime.tailEvents(id, cursor)
                    val status = runtime.pollStatus(id)
                    events to status
                }
            }
            step.onSuccess { (events, status) ->
                pollingError = null
                if (events.isNotEmpty()) {
                    lines = lines + events.map(::renderEvent)
                    cursor = events.last().seq
                }
                presentation = presentRun(status.phase, status.workerActive, status.activity)
                finalReport = status.finalReport
            }.onFailure {
                pollingError = operationError(UiOperation.OBSERVE_RUN, it)
            }
            if (presentation?.keepPolling == false) break
            delay(300)
        }
    }

    LaunchedEffect(showTechnicalEvents, lines.size) {
        if (showTechnicalEvents && lines.isNotEmpty()) listState.animateScrollToItem(lines.size - 1)
    }

    if (confirmCancel && runId != null) {
        AlertDialog(
            onDismissRequest = { confirmCancel = false },
            title = { Text("Cancel this run?") },
            text = { Text("The run will stop at its next safe boundary. Remote actions may already have happened. Cancelling does not undo them.") },
            confirmButton = {
                TextButton(enabled = !requesting && presentation?.canCancel == true, onClick = {
                    confirmCancel = false
                    error = null
                    requesting = true
                    scope.launch {
                        catchingOperation { withContext(Dispatchers.IO) { runtime.cancel(runId) } }
                            .onFailure { error = operationError(UiOperation.CANCEL_RUN, it) }
                        requesting = false
                    }
                }) { Text("Cancel run") }
            },
            dismissButton = { TextButton(onClick = { confirmCancel = false }) { Text("Keep run") } },
        )
    }

    Column(Modifier.fillMaxSize().verticalScroll(inputScroll).padding(16.dp)) {
        Text(setup?.title ?: flow?.id ?: "Run ${runId.orEmpty()}", style = MaterialTheme.typography.headlineSmall)
        Text(
            flow?.let { "sha256 ${it.sha256.take(12)}…" } ?: runId.orEmpty(),
            fontFamily = FontFamily.Monospace,
            style = MaterialTheme.typography.bodySmall,
        )

        if (runId == null && flow != null) {
            setup?.description?.let { Text(it) }
            if (setup != null) {
                Button(enabled = !starting, onClick = {
                    // Setup can change model/endpoint overrides. Require Continue
                    // to supply fresh verified settings before another start.
                    configurationJson = null
                    preparing = true
                }) {
                    Text(if (configurationJson == null) "Prepare models" else "Model setup")
                }
            }
            val fields = setup?.inputFields.orEmpty()
            val fieldErrors = if (inputAttempted) StructuredInput.errors(fields, structuredValues) else emptyMap()
            if (fields.isEmpty()) {
                OutlinedTextField(
                    value = userInput,
                    onValueChange = { userInput = it },
                    label = { Text(setup?.inputLabel ?: "Input") },
                    placeholder = { setup?.inputPlaceholder?.let { Text(it) } },
                    modifier = Modifier.fillMaxWidth().padding(vertical = 16.dp),
                )
            } else {
                fields.forEach { field ->
                    val fieldError = fieldErrors[field.key]
                    OutlinedTextField(
                        value = structuredValues[field.key].orEmpty(),
                        onValueChange = { structuredValues = structuredValues + (field.key to it) },
                        enabled = !starting,
                        label = { Text(field.label) },
                        placeholder = { field.placeholder?.let { Text(it) } },
                        isError = fieldError != null,
                        supportingText = { fieldError?.let { Text(it) } },
                        singleLine = field.kind == "url",
                        keyboardOptions = KeyboardOptions(keyboardType = if (field.kind == "url") KeyboardType.Uri else KeyboardType.Text),
                        modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                    )
                }
            }
            Button(enabled = !starting && setupChecked && (setup == null || configurationJson != null), onClick = {
                inputAttempted = true
                val validation = StructuredInput.errors(fields, structuredValues)
                if (validation.isNotEmpty()) {
                    error = UiError("Check the input fields before starting.")
                    return@Button
                }
                val submittedInput = try {
                    StructuredInput.serialize(fields, structuredValues, userInput)
                } catch (failure: IllegalArgumentException) {
                    error = operationError(UiOperation.PREPARE_INPUT, failure)
                    return@Button
                } catch (failure: Exception) {
                    error = operationError(UiOperation.PREPARE_INPUT, failure)
                    return@Button
                }
                val submittedConfiguration = configurationJson
                error = null
                starting = true
                prepareNotifications()
                scope.launch {
                    catchingOperation {
                        withContext(Dispatchers.IO) {
                            check(!runtime.hasActiveRun()) { "Finish or cancel the active run before starting another." }
                            val id = submittedConfiguration?.let {
                                runtime.startRunConfigured(flow.sha256, submittedInput, it)
                            } ?: runtime.startRun(flow.sha256, submittedInput)
                            // Preserve the successful UUID even if storage/service setup fails.
                            val storageError = catchingOperation { RunHistory(context).record(id) }.exceptionOrNull()
                            val serviceError = catchingOperation { RunService.start(context.applicationContext, id) }.exceptionOrNull()
                            id to combineUiErrors(
                                storageError?.let { operationError(UiOperation.SAVE_HISTORY, it) },
                                serviceError?.let { operationError(UiOperation.BACKGROUND_PROGRESS, it) },
                            )
                        }
                    }.onSuccess { (id, setupError) ->
                        onRunStarted(id)
                        startedRunError = setupError
                    }.onFailure { error = operationError(UiOperation.START_RUN, it) }
                    starting = false
                }
            }) { Text(if (starting) "Starting…" else "Start run") }
        } else if (runId != null) {
            Text(presentation?.text ?: "Loading run status…", style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.padding(vertical = 8.dp).semantics { liveRegion = LiveRegionMode.Polite })

            presentation?.details?.let { TechnicalDetails(it) }

            FlowRow(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Button(enabled = !requesting && presentation?.canPause == true, onClick = {
                    error = null
                    requesting = true
                    scope.launch {
                        catchingOperation { withContext(Dispatchers.IO) { runtime.pause(runId) } }
                            .onFailure { error = operationError(UiOperation.PAUSE_RUN, it) }
                        requesting = false
                    }
                }) { Text("Pause") }
                Button(enabled = !requesting && presentation?.canResume == true, onClick = {
                    error = null
                    requesting = true
                    prepareNotifications()
                    scope.launch {
                        var resumed = false
                        catchingOperation {
                            withContext(Dispatchers.IO) {
                                runtime.resume(runId)
                                resumed = true
                                RunService.start(context.applicationContext, runId)
                            }
                        }.onFailure { error = operationError(if (resumed) UiOperation.BACKGROUND_PROGRESS else UiOperation.RESUME_RUN, it) }
                        if (resumed) {
                            // The old disk-only observer stopped. Restart it only
                            // after this explicit user request restores a worker.
                            presentation = null
                            pollingGeneration++
                        }
                        requesting = false
                    }
                }) { Text("Resume") }
                Button(enabled = !requesting && presentation?.canCancel == true, onClick = {
                    confirmCancel = true
                }) { Text("Cancel run") }
            }

            finalReport?.let { report ->
                Text("Final report", style = MaterialTheme.typography.titleMedium)
                SelectionContainer { Text(report, modifier = Modifier.padding(top = 4.dp)) }
                TextButton(onClick = {
                    scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Final report", report))) }
                }) { Text("Copy final report") }
            }

            Button(
                onClick = { showTechnicalEvents = !showTechnicalEvents },
                modifier = Modifier.semantics { stateDescription = if (showTechnicalEvents) "Expanded" else "Collapsed" },
            ) { Text(if (showTechnicalEvents) "Hide technical events" else "Show technical events") }
            if (showTechnicalEvents) {
                TextButton(enabled = lines.isNotEmpty(), onClick = {
                    val events = lines.joinToString("\n")
                    scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Technical events", events))) }
                }) { Text("Copy technical events") }
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxWidth().heightIn(max = 240.dp).padding(vertical = 8.dp),
                ) {
                    items(lines) { line ->
                        SelectionContainer {
                            Text(line, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        }

        if (!notificationsEnabled && (runId != null || starting)) {
            Text("Background progress notifications are disabled. Enable them in Android app settings.", style = MaterialTheme.typography.bodySmall)
        }
        error?.let { OperationError(it) }
        pollingError?.let { OperationError(it) }
        startedRunError?.let { OperationError(it) }
        Button(onClick = onBack, modifier = Modifier.padding(top = 8.dp)) { Text("Back") }
    }
}
