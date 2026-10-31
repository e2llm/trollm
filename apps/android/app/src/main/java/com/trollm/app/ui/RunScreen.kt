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
import androidx.compose.material3.ButtonDefaults
import androidx.compose.foundation.BorderStroke
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import com.trollm.app.TechnicalEvents
import com.trollm.app.RunService
import com.trollm.app.StatusTone
import com.trollm.app.RunPresentation
import com.trollm.app.presentRun
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.RunPhaseApi
import uniffi.trollm_uniffi.TrollmRuntime

/**
 * Run screen: user input → `start_run` → tail the event log via
 * `observe_run` polling. Progress lines come from event types
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
    sourceInstallation: String? = null,
    sourceAssociation: String? = null,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboard.current
    var userInput by rememberSaveable(flow?.sha256, sourceInstallation) { mutableStateOf("") }
    val valuesSaver = remember { mapSaver<Map<String, String>>(
        save = { it }, restore = { saved -> saved.mapValues { it.value as? String ?: "" } },
    ) }
    var structuredValues by rememberSaveable(flow?.sha256, sourceInstallation, stateSaver = valuesSaver) { mutableStateOf(emptyMap<String, String>()) }
    var inputAttempted by remember(flow?.sha256, sourceInstallation) { mutableStateOf(false) }
    val runId = existingRunId
    val scope = rememberCoroutineScope()
    var starting by remember(flow?.sha256, sourceInstallation) { mutableStateOf(false) }
    var setup by remember(flow?.sha256, sourceInstallation) { mutableStateOf<FlowSetup?>(null) }
    var setupChecked by remember(flow?.sha256, sourceInstallation) { mutableStateOf(false) }
    var preparing by rememberSaveable(flow?.sha256, sourceInstallation) { mutableStateOf(false) }
    var configurationJson by rememberSaveable(flow?.sha256, sourceInstallation) { mutableStateOf<String?>(null) }
    var sourceReportTerminal by remember(runId) { mutableStateOf(false) }
    var finalReport by remember(runId) { mutableStateOf<String?>(null) }
    var olderEventsOmitted by remember(runId) { mutableStateOf(false) }
    var lines by remember(runId) { mutableStateOf<List<String>>(emptyList()) }
    var error by remember(runId, flow?.sha256) { mutableStateOf<UiError?>(null) }
    // Keep post-start warnings when the successful UUID changes the screen state.
    var startedRunError by remember(flow?.sha256, sourceInstallation) { mutableStateOf<UiError?>(null) }
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

    LaunchedEffect(flow?.sha256, sourceInstallation) {
        val selected = flow ?: return@LaunchedEffect
        catchingOperation {
            withContext(Dispatchers.IO) { (sourceInstallation?.let { runtime.sourceBundleSetup(it) } ?: if (sourceInstallation == null) runtime.flowSetup(selected.sha256) else null)?.let { FlowSetup.parse(it) } }
        }.onSuccess {
            setup = it
            setupChecked = true
        }.onFailure { error = operationError(UiOperation.LOAD_SETUP, it) }
    }

    if (preparing && flow != null && runId == null) {
        FlowSetupScreen(
            runtime = runtime,
            flowSha = sourceInstallation ?: flow.sha256,
            sourceInstallation = sourceInstallation,
            onReady = { configurationJson = it; preparing = false },
            onBack = { preparing = false },
        )
        return
    }

    // Bounded worker-owned observation; strict journal APIs remain separate.
    LaunchedEffect(runId, pollingGeneration) {
        val id = runId ?: return@LaunchedEffect
        var cursor = 0uL
        lines = emptyList()
        olderEventsOmitted = false
        while (isActive) {
            val step = catchingOperation {
                withContext(Dispatchers.IO) {
                    runtime.observeRun(id, cursor)
                }
            }
            step.onSuccess { observation ->
                val status = observation.status
                pollingError = null
                val incoming = observation.events.map(::renderEvent)
                val retained = TechnicalEvents.append(lines, incoming)
                olderEventsOmitted = olderEventsOmitted || observation.olderEventsOmitted ||
                    retained.size < lines.size + incoming.size
                lines = retained
                cursor = observation.nextSeq
                presentation = presentRun(status.phase, status.workerActive, status.activity)
                finalReport = status.finalReport
                sourceReportTerminal = !status.workerActive && (status.phase is RunPhaseApi.Succeeded || status.phase is RunPhaseApi.Failed)
            }.onFailure {
                pollingError = operationError(UiOperation.OBSERVE_RUN, it)
            }
            if (presentation?.keepPolling == false && step.getOrNull()?.hasMore != true) break
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

    Column(
        Modifier.fillMaxSize().verticalScroll(inputScroll).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        ScreenHeader(setup?.title ?: flow?.id ?: "Run", onBack = onBack)

        if (runId == null && flow != null) {
            setup?.description?.let { Text(it) }
            if (setup != null) {
                OutlinedButton(enabled = !starting, shape = MaterialTheme.shapes.medium, onClick = {
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
            Button(shape = MaterialTheme.shapes.medium, enabled = !starting && setupChecked && (setup == null || configurationJson != null), onClick = {
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
                val declaredSource = sourceInstallation != null && org.json.JSONObject(checkNotNull(sourceAssociation))
                    .getJSONObject("descriptor").getJSONObject("source").opt("origin") == "DECLARED"
                val originalInput = try {
                    if (declaredSource) StructuredInput.serializeOriginal(fields, structuredValues) else submittedInput
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
                            if (sourceInstallation != null) com.trollm.app.MoraySource.authorizeFreshStart(
                                context, org.json.JSONObject(checkNotNull(sourceAssociation)))
                            val admittedAssociation = if (declaredSource) {
                                val admission = runtime.admitSourceInput(checkNotNull(sourceInstallation), originalInput, submittedInput)
                                org.json.JSONObject(checkNotNull(sourceAssociation))
                                    .put("inputAdmission", org.json.JSONObject(admission)).toString()
                            } else sourceAssociation
                            val executionInput = if (declaredSource) originalInput else submittedInput
                            val id = submittedConfiguration?.let {
                                if (sourceInstallation == null) runtime.startRunConfigured(flow.sha256, submittedInput, it)
                                else runtime.startSourceRunConfigured(flow.sha256, sourceInstallation, executionInput, it, checkNotNull(admittedAssociation))
                            } ?: if (sourceInstallation == null) runtime.startRun(flow.sha256, submittedInput)
                                else runtime.startSourceRun(flow.sha256, sourceInstallation, executionInput, checkNotNull(admittedAssociation))
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
            StatusPanel(
                title = presentation?.label ?: "Run status",
                message = presentation?.text?.takeIf { it != presentation?.label } ?: if (presentation == null) "Loading run status…" else null,
                tone = presentation?.tone ?: StatusTone.NEUTRAL,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )

            FlowRow(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                OutlinedButton(shape = MaterialTheme.shapes.medium, enabled = !requesting && presentation?.canPause == true, onClick = {
                    error = null
                    requesting = true
                    scope.launch {
                        catchingOperation { withContext(Dispatchers.IO) { runtime.pause(runId) } }
                            .onFailure { error = operationError(UiOperation.PAUSE_RUN, it) }
                        requesting = false
                    }
                }) { Text("Pause") }
                OutlinedButton(shape = MaterialTheme.shapes.medium, enabled = !requesting && presentation?.canResume == true, onClick = {
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
                OutlinedButton(
                    shape = MaterialTheme.shapes.medium,
                    enabled = !requesting && presentation?.canCancel == true,
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    border = BorderStroke(1.dp, if (!requesting && presentation?.canCancel == true)
                        MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.outline.copy(alpha = 0.38f)),
                    onClick = { confirmCancel = true },
                ) { Text("Cancel run") }
            }

            SourceRunReportPanel(runtime, runId, sourceReportTerminal)

            finalReport?.let { report ->
                ContentCard {
                    SectionHeading("Final report")
                    SelectionContainer { Text(report, style = MaterialTheme.typography.bodyLarge) }
                    Button(
                        modifier = Modifier.fillMaxWidth(),
                        shape = MaterialTheme.shapes.medium,
                        onClick = {
                            scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Final report", report))) }
                        },
                    ) { Text("Copy final report") }
                }
            }

            TextButton(
                onClick = { showTechnicalEvents = !showTechnicalEvents },
                modifier = Modifier.semantics { stateDescription = if (showTechnicalEvents) "Expanded" else "Collapsed" },
            ) { Text(if (showTechnicalEvents) "Hide technical details" else "Technical details") }
            if (showTechnicalEvents) {
                if (olderEventsOmitted) Text("Older technical events omitted from this view.")
                SelectionContainer {
                    Text(
                        "Run ID: $runId" + (flow?.let { "\nFlow ID: ${it.id}\nBundle SHA-256: ${it.sha256}" } ?: ""),
                        fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                presentation?.details?.let { details ->
                    SelectionContainer { Text(details, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
                }
                SectionHeading("Technical events")
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
        if (runId == null && flow != null) {
            TechnicalDetails("Flow ID: ${flow.id}\nBundle SHA-256: ${flow.sha256}")
        }
    }
}
