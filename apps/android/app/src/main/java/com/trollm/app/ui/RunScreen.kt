package com.trollm.app.ui

import android.Manifest
import android.content.ClipData
import android.content.Context
import android.text.format.DateFormat
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.mapSaver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import com.trollm.app.BundledStarter
import com.trollm.app.FlowSetup
import com.trollm.app.RunCommand
import com.trollm.app.RunService
import com.trollm.app.RunStartPhase
import com.trollm.app.RunStartRequest
import com.trollm.app.RunStartViewModel
import com.trollm.app.StarterFlows
import com.trollm.app.StatusTone
import com.trollm.app.StructuredInput
import com.trollm.app.TechnicalEvents
import com.trollm.app.UiError
import com.trollm.app.UiOperation
import com.trollm.app.catchingOperation
import com.trollm.app.operationError
import com.trollm.app.presentRun
import com.trollm.app.runFailureRecovery
import java.util.Date
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.RunPhaseApi
import uniffi.trollm_uniffi.RunStatusInfo
import uniffi.trollm_uniffi.RunSummaryInfo
import uniffi.trollm_uniffi.TrollmRuntime

@Composable
internal fun RunScreen(
    runtime: TrollmRuntime,
    flow: FlowInfo?,
    existingRunId: String?,
    onRunStarted: (String) -> Unit,
    onBack: () -> Unit,
    runScreenKey: String,
    runStartViewModel: RunStartViewModel,
    sourceInstallation: String? = null,
    sourceAssociation: String? = null,
    onOpenConnection: (String, String) -> Unit = { _, _ -> },
) {
    val context = LocalContext.current
    val startRecord by runStartViewModel.record.collectAsState()
    val startPhase = startRecord?.takeIf { it.screenKey == runScreenKey }?.phase
    val started = startPhase as? RunStartPhase.Started
    val unconfirmed = startPhase as? RunStartPhase.Unconfirmed
    val reportedStart = (startRecord?.phase as? RunStartPhase.Started)?.result
    val notifyRunStarted by rememberUpdatedState(onRunStarted)
    LaunchedEffect(runScreenKey, existingRunId, started) {
        if (existingRunId == null && started != null && !started.acknowledged) {
            notifyRunStarted(started.result.runId)
        }
    }
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

    if (existingRunId != null) {
        SavedRunScreen(
            runtime, existingRunId, flow, sourceInstallation,
            reportedStart?.takeIf { it.runId == existingRunId }?.warning,
            notificationsEnabled, prepareNotifications, onBack,
        )
        return
    }
    if (flow == null) return

    var userInput by rememberSaveable(flow.sha256, sourceInstallation) { mutableStateOf("") }
    val valuesSaver = remember { mapSaver<Map<String, String>>(
        save = { it }, restore = { saved -> saved.mapValues { it.value as? String ?: "" } },
    ) }
    var structuredValues by rememberSaveable(flow.sha256, sourceInstallation, stateSaver = valuesSaver) {
        mutableStateOf(emptyMap<String, String>())
    }
    var inputEdited by rememberSaveable(flow.sha256, sourceInstallation) { mutableStateOf(false) }
    var inputAttempted by rememberSaveable(flow.sha256, sourceInstallation) { mutableStateOf(false) }
    val starting = startPhase == RunStartPhase.Pending || (started != null && !started.acknowledged)
    // Keep the requesting destination present while the activity owner records the UUID.
    BackHandler(enabled = starting) {}
    var setup by remember(flow.sha256, sourceInstallation) { mutableStateOf<FlowSetup?>(null) }
    var setupChecked by remember(flow.sha256, sourceInstallation) { mutableStateOf(false) }
    var error by remember(flow.sha256, sourceInstallation) { mutableStateOf<UiError?>(null) }
    val cachedStarter = if (sourceInstallation == null) StarterFlows.verifiedKind(flow.sha256) else null
    var stock by remember(flow.sha256, sourceInstallation) { mutableStateOf(cachedStarter) }
    var stockChecked by remember(flow.sha256, sourceInstallation) {
        mutableStateOf(sourceInstallation != null || cachedStarter != null)
    }
    val starter = stock
    var hasDeclaredModelSlots by remember(flow.sha256, sourceInstallation) { mutableStateOf<Boolean?>(null) }

    LaunchedEffect(runtime, flow.sha256, sourceInstallation) {
        if (sourceInstallation == null && !stockChecked) {
            stock = catchingOperation {
                StarterFlows.resolveKind(context.applicationContext, runtime, flow.sha256)
            }.getOrNull()
            stockChecked = true
        }
    }
    LaunchedEffect(runtime, flow.sha256, sourceInstallation, setupChecked, setup) {
        if (setupChecked && setup == null) {
            hasDeclaredModelSlots = catchingOperation {
                withContext(Dispatchers.IO) { runtime.flowHasDeclaredModelSlots(flow.sha256, sourceInstallation) }
            }.getOrNull()
        }
    }

    LaunchedEffect(runtime, flow.sha256, sourceInstallation) {
        catchingOperation {
            withContext(Dispatchers.IO) {
                val json = if (sourceInstallation == null) runtime.flowSetup(flow.sha256)
                    else runtime.sourceBundleSetup(sourceInstallation)
                json?.let { FlowSetup.parse(it) }
            }
        }.onSuccess {
            setup = it
            setupChecked = true
        }.onFailure { error = operationError(UiOperation.LOAD_SETUP, it) }
    }

    val descriptor = setup
    val fields = descriptor?.inputFields.orEmpty()
    val ownerBusy = when (val otherPhase = startRecord?.takeUnless { it.screenKey == runScreenKey }?.phase) {
        RunStartPhase.Pending -> true
        is RunStartPhase.Started -> !otherPhase.acknowledged
        else -> false
    }
    val focusers = remember(fields) { fields.associate { it.key to FocusRequester() } }
    val fieldErrors = if (inputAttempted) StructuredInput.errors(fields, structuredValues) else emptyMap()
    val startRun: (String?) -> Unit = startRun@{ submittedConfiguration ->
        if (starting || ownerBusy || unconfirmed != null) return@startRun
        inputAttempted = true
        val validation = StructuredInput.errors(fields, structuredValues)
        if (validation.isNotEmpty()) {
            error = null
            focusers[validation.keys.first()]?.requestFocus()
            return@startRun
        }
        val submittedInput = try {
            StructuredInput.serialize(fields, structuredValues, userInput)
        } catch (failure: IllegalArgumentException) {
            error = operationError(UiOperation.PREPARE_INPUT, failure)
            return@startRun
        } catch (failure: Exception) {
            error = operationError(UiOperation.PREPARE_INPUT, failure)
            return@startRun
        }
        val declaredSource: Boolean
        val originalInput = try {
            declaredSource = sourceInstallation != null && JSONObject(checkNotNull(sourceAssociation))
                .getJSONObject("descriptor").getJSONObject("source").opt("origin") == "DECLARED"
            if (declaredSource) StructuredInput.serializeOriginal(fields, structuredValues) else submittedInput
        } catch (failure: Exception) {
            error = operationError(UiOperation.PREPARE_INPUT, failure)
            return@startRun
        }
        error = null
        val request = RunStartRequest(
            flowSha = flow.sha256,
            sourceInstallation = sourceInstallation,
            sourceAssociation = sourceAssociation,
            submittedInput = submittedInput,
            originalInput = originalInput,
            configuration = submittedConfiguration,
            declaredSource = declaredSource,
        )
        if (runStartViewModel.start(runScreenKey, context.applicationContext, runtime, request)) {
            prepareNotifications()
        }
    }
    val form: @Composable ColumnScope.() -> Unit = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (fields.isEmpty()) {
                OutlinedTextField(
                    value = userInput,
                    onValueChange = { userInput = it; inputEdited = true },
                    enabled = !starting,
                    label = { Text(descriptor?.inputLabel ?: "Input") },
                    placeholder = { descriptor?.inputPlaceholder?.let { Text(it) } },
                    minLines = 4,
                    modifier = Modifier.fillMaxWidth(),
                )
            } else {
                fields.forEach { field ->
                    val fieldError = fieldErrors[field.key]
                    val isUrl = field.kind == "url"
                    OutlinedTextField(
                        value = structuredValues[field.key].orEmpty(),
                        onValueChange = {
                            structuredValues = structuredValues + (field.key to it)
                            inputEdited = true
                        },
                        enabled = !starting,
                        label = { Text(field.label) },
                        placeholder = { field.placeholder?.let { Text(it) } },
                        isError = fieldError != null,
                        supportingText = {
                            val need = if (field.required) "Required" else "Optional"
                            Text(fieldError ?: if (isUrl) "$need · HTTP or HTTPS URL" else need)
                        },
                        singleLine = isUrl,
                        keyboardOptions = KeyboardOptions(keyboardType = if (isUrl) KeyboardType.Uri else KeyboardType.Text),
                        modifier = Modifier.fillMaxWidth().focusRequester(focusers.getValue(field.key)),
                    )
                }
            }
            if (!inputEdited && (starter == BundledStarter.TEXT_SUMMARY || starter == BundledStarter.CHECK_TOTAL)) {
                TextButton(enabled = !starting, onClick = {
                    if (starter == BundledStarter.TEXT_SUMMARY) {
                        userInput = "The library will open an hour later on Friday for staff training. Books can still be returned through the outside slot."
                    } else {
                        structuredValues = mapOf("text" to "Two items cost 39.99 and 40.10.", "budget" to "80")
                    }
                    inputEdited = true
                }) { Text("Use editable example") }
            }
            if (!notificationsEnabled && starting) NotificationNotice()
            (error ?: (startPhase as? RunStartPhase.NotDispatched)?.error)?.let { OperationError(it) }
        }
    }
    if (unconfirmed != null || ownerBusy) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            ScreenHeader(descriptor?.title ?: flow.id, onBack = onBack)
            if (unconfirmed != null) {
                StatusPanel(
                    "Start could not be confirmed",
                    "A run may already have started. Check run history for a record of the request.",
                    StatusTone.WARNING,
                    Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
            } else {
                StatusPanel(
                    "A previous start request is still being recorded",
                    tone = StatusTone.INFO,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
            }
            Button(onClick = onBack, modifier = Modifier.fillMaxWidth()) { Text("Review run history") }
            unconfirmed?.diagnostic?.let { TechnicalDetails(it) }
        }
    } else if (descriptor != null && stockChecked) {
        FlowSetupScreen(
            pinned = descriptor,
            flow = flow,
            sourceInstallation = sourceInstallation,
            starter = starter,
            starting = starting,
            onStart = startRun,
            onBack = onBack.takeUnless { starting },
            onOpenConnection = { name, endpoint -> if (!starting) onOpenConnection(name, endpoint) },
            inputs = form,
        )
    } else {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            ScreenHeader(descriptor?.title ?: flow.id, onBack = onBack.takeUnless { starting })
            // Wait for verified metadata before presenting the matching form and actions.
            if (setupChecked) form()
            when {
                starting -> StatusPanel("Starting run", tone = StatusTone.INFO)
                setupChecked && stockChecked -> {
                    if (hasDeclaredModelSlots == false) {
                        StatusPanel("No model files required", tone = StatusTone.SUCCESS)
                    }
                    Button(modifier = Modifier.fillMaxWidth(), onClick = { startRun(null) }) { Text("Start run") }
                }
                error == null -> StatusPanel("Checking flow setup", tone = StatusTone.INFO)
                !setupChecked -> error?.let { OperationError(it) }
            }
            TechnicalDetails("Flow ID: ${flow.id}\nBundle SHA-256: ${flow.sha256}")
        }
    }
}

private data class RunIdentity(
    val summary: RunSummaryInfo?,
    val title: String?,
    val source: Boolean?,
)

private data class PendingRunCommand(val command: RunCommand, val before: ULong)

private val pendingCommandSaver = mapSaver<PendingRunCommand?>(
    save = { pending -> pending?.let { mapOf("command" to it.command.name, "sequence" to it.before.toString()) }.orEmpty() },
    restore = { saved ->
        val command = RunCommand.entries.firstOrNull { it.name == saved["command"] }
        val sequence = (saved["sequence"] as? String)?.toULongOrNull()
        if (command != null && sequence != null) PendingRunCommand(command, sequence) else null
    },
)

@Composable
private fun SavedRunScreen(
    runtime: TrollmRuntime,
    runId: String,
    flow: FlowInfo?,
    sourceInstallation: String?,
    startedRunError: UiError?,
    notificationsEnabled: Boolean,
    prepareNotifications: () -> Unit,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboard.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    var identity by remember(runId) { mutableStateOf<RunIdentity?>(null) }
    var observation by remember(runId) { mutableStateOf<RunStatusInfo?>(null) }
    var pollingError by remember(runId) { mutableStateOf<UiError?>(null) }
    var error by remember(runId) { mutableStateOf<UiError?>(null) }
    var lines by remember(runId) { mutableStateOf<List<String>>(emptyList()) }
    var cursor by remember(runId) { mutableStateOf(0uL) }
    var olderEventsOmitted by remember(runId) { mutableStateOf(false) }
    var lastStep by remember(runId) { mutableStateOf<String?>(null) }
    var showTechnicalEvents by rememberSaveable(runId) { mutableStateOf(false) }
    var showSourceReport by rememberSaveable(runId) { mutableStateOf(false) }
    var confirmCancel by remember(runId) { mutableStateOf(false) }
    var acceptedCommand by rememberSaveable(runId, stateSaver = pendingCommandSaver) {
        mutableStateOf<PendingRunCommand?>(null)
    }
    var submittingCommand by remember(runId) { mutableStateOf<PendingRunCommand?>(null) }
    var pollingGeneration by remember(runId) { mutableStateOf(0) }

    LaunchedEffect(runtime, runId, sourceInstallation, flow?.sha256) {
        identity = withContext(Dispatchers.IO) {
            val summary = catchingOperation { runtime.runSummary(runId) }.getOrNull()
            val isSource = catchingOperation { runtime.isSourceRun(runId) }.getOrNull()
            val installation = sourceInstallation ?: if (isSource == true) catchingOperation {
                JSONObject(runtime.sourceRunAssociation(runId)).optJSONObject("installation")
                    ?.optString("installationSha256")?.takeIf(String::isNotBlank)
            }.getOrNull() else null
            val title = catchingOperation {
                val setupJson = if (installation != null) runtime.sourceBundleSetup(installation)
                    else (summary?.flowSha256 ?: flow?.sha256)?.let(runtime::flowSetup)
                setupJson?.let { FlowSetup.parse(it).title }
            }.getOrNull()
            RunIdentity(summary, title, isSource)
        }
    }

    LaunchedEffect(runtime, runId, pollingGeneration, lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            observation = null
            while (isActive) {
                val next = catchingOperation { withContext(Dispatchers.IO) { runtime.observeRun(runId, cursor) } }
                next.onSuccess { result ->
                    val incoming = result.events.map { event ->
                        if (event.eventType == "node_entered") {
                            val node = catchingOperation { JSONObject(event.json).opt("node") as? String }.getOrNull()
                            if (!node.isNullOrBlank()) lastStep = node
                        }
                        "#${event.seq} ${renderEvent(event)}"
                    }
                    val retained = TechnicalEvents.append(lines, incoming)
                    olderEventsOmitted = olderEventsOmitted || result.olderEventsOmitted ||
                        retained.size < lines.size + incoming.size
                    lines = retained
                    cursor = result.nextSeq
                    val status = result.status
                    val command = acceptedCommand
                    if (command != null &&
                        command.command.acknowledged(status.phase, status.workerActive, command.before, status.lastSeq)) {
                        acceptedCommand = null
                    }
                    observation = status
                    pollingError = null
                }.onFailure {
                    observation = null
                    pollingError = operationError(UiOperation.OBSERVE_RUN, it)
                }
                val result = next.getOrNull()
                if (result != null && !result.status.workerActive && !result.hasMore && acceptedCommand == null && submittingCommand == null) break
                delay(300)
            }
        }
    }

    val status = observation
    val presentation = status?.let { presentRun(it.phase, it.workerActive, it.activity) }
    val pending = submittingCommand ?: acceptedCommand
    val controlsReady = pending == null
    val requestCommand: (RunCommand) -> Unit = requestCommand@{ command ->
        val latest = observation ?: return@requestCommand
        if (acceptedCommand != null || submittingCommand != null) return@requestCommand
        val current = presentRun(latest.phase, latest.workerActive, latest.activity)
        val allowed = when (command) {
            RunCommand.PAUSE -> current.canPause
            RunCommand.CANCEL -> current.canCancel
            RunCommand.RESUME -> current.canResume
        }
        if (!allowed) return@requestCommand
        error = null
        val requested = PendingRunCommand(command, latest.lastSeq)
        submittingCommand = requested
        if (command == RunCommand.RESUME) prepareNotifications()
        scope.launch {
            var resumed = false
            val result = catchingOperation {
                withContext(Dispatchers.IO) {
                    when (command) {
                        RunCommand.PAUSE -> runtime.pause(runId)
                        RunCommand.CANCEL -> runtime.cancel(runId)
                        RunCommand.RESUME -> {
                            runtime.resume(runId)
                            resumed = true
                            RunService.start(context.applicationContext, runId)
                        }
                    }
                }
            }.onFailure {
                val operation = when (command) {
                    RunCommand.PAUSE -> UiOperation.PAUSE_RUN
                    RunCommand.CANCEL -> UiOperation.CANCEL_RUN
                    RunCommand.RESUME -> if (resumed) UiOperation.BACKGROUND_PROGRESS else UiOperation.RESUME_RUN
                }
                error = operationError(operation, it)
            }
            if (result.isSuccess || resumed) acceptedCommand = requested
            // An inactive observation loop may have stopped while the record was saved.
            if (resumed) {
                observation = null
                pollingGeneration++
            }
            submittingCommand = null
        }
    }

    if (confirmCancel) {
        AlertDialog(
            onDismissRequest = { confirmCancel = false },
            title = { Text("Cancel this run?") },
            text = { Text("The run will stop at its next safe boundary. Remote actions may already have happened. Cancelling does not undo them.") },
            confirmButton = {
                TextButton(
                    enabled = controlsReady && presentation?.canCancel == true,
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                    onClick = {
                        confirmCancel = false
                        requestCommand(RunCommand.CANCEL)
                    },
                ) { Text("Cancel run") }
            },
            dismissButton = { TextButton(onClick = { confirmCancel = false }) { Text("Keep run") } },
        )
    }

    val summary = identity?.summary
    val flowSha = summary?.flowSha256 ?: flow?.sha256
    val flowId = summary?.flowId ?: flow?.id
    val title = identity?.title ?: flowId ?: "Saved run"
    val eligibleStockSha = flowSha?.takeIf { sourceInstallation == null && identity?.source == false }
    var verifiedStarter by remember(eligibleStockSha) {
        mutableStateOf(eligibleStockSha?.let(StarterFlows::verifiedKind))
    }
    LaunchedEffect(runtime, eligibleStockSha) {
        if (eligibleStockSha != null && verifiedStarter == null) {
            verifiedStarter = catchingOperation {
                StarterFlows.resolveKind(context.applicationContext, runtime, eligibleStockSha)
            }.getOrNull()
        }
    }
    val localStart = summary?.startedAtMs?.takeIf { it > 0uL && it <= Long.MAX_VALUE.toULong() }?.let {
        val date = Date(it.toLong())
        "Started ${DateFormat.getDateFormat(context).format(date)} at ${DateFormat.getTimeFormat(context).format(date)}"
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(24.dp),
    ) {
        ScreenHeader(
            title = title,
            onBack = onBack,
            subtitle = localStart,
            actions = if (controlsReady && presentation?.canCancel == true) {
                {
                    OverflowMenu("Run actions") { dismiss ->
                        DropdownMenuItem(text = { Text("Cancel run") }, onClick = {
                            dismiss()
                            confirmCancel = true
                        })
                    }
                }
            } else null,
        )

        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
                pollingError != null -> StatusPanel(
                    "Progress unavailable", "The worker may still be active.", StatusTone.WARNING,
                    Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                    prominent = true,
                )
                presentation == null -> StatusPanel(
                    "Checking saved progress", tone = StatusTone.INFO,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
                presentation.activityLabel != null -> {
                    StatusPanel(presentation.activityLabel, tone = StatusTone.INFO,
                        modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                        prominent = true)
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                }
                else -> StatusPanel(
                    presentation.label,
                    when (status?.phase) {
                        is RunPhaseApi.Succeeded -> "The flow's completion checks passed."
                        else -> when (presentation.label) {
                            "Interrupted" -> "Restore progress using the saved run."
                            "Paused" -> (status?.phase as? RunPhaseApi.Paused)?.reason
                            else -> presentation.text.takeIf { it != presentation.label }
                        }
                    },
                    presentation.tone,
                    Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
            }
            pending?.let { Text(it.command.pendingText, style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
            if (presentation?.isTerminal != true && (presentation != null || lastStep != null)) {
                SectionHeading("Last recorded step")
                Text(
                    lastStep ?: if (olderEventsOmitted) "No step in the available events." else "No step recorded yet.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            if (presentation?.isTerminal == true) {
                val report = status?.finalReport
                if (report != null) {
                    Surface(shape = MaterialTheme.shapes.large, color = MaterialTheme.colorScheme.surface) {
                        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            SectionHeading("Final report")
                            if (verifiedStarter != null) Text("Written by a model", style = MaterialTheme.typography.bodySmall)
                            SelectionContainer { Text(report, style = MaterialTheme.typography.bodyLarge) }
                        }
                    }
                    Button(
                        modifier = Modifier.fillMaxWidth(),
                        onClick = { scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Final report", report))) } },
                    ) { Text("Copy final report") }
                } else {
                    Text("No final report is available for this run.", style = MaterialTheme.typography.bodyMedium)
                }
                if (presentation.tone == StatusTone.ERROR) {
                    runFailureRecovery(verifiedStarter)?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                    if (!showTechnicalEvents) Button(
                        onClick = { showTechnicalEvents = true },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("Review saved details") }
                }
            } else if (presentation != null) {
                if (presentation.canResume && controlsReady) {
                    Button(onClick = { requestCommand(RunCommand.RESUME) }, modifier = Modifier.fillMaxWidth()) {
                        Text("Resume saved run")
                    }
                }
                if (presentation.canPause && controlsReady) {
                    OutlinedButton(onClick = { requestCommand(RunCommand.PAUSE) }) { Text("Pause run") }
                }
                Text("You can leave this screen and reopen this run from history.", style = MaterialTheme.typography.bodyMedium)
            }
        }

        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            DisclosureRow("Technical details", showTechnicalEvents) { showTechnicalEvents = !showTechnicalEvents }
            if (showTechnicalEvents) {
                val identifiers = listOfNotNull(
                    "Run ID · $runId",
                    flowId?.let { "Flow ID · $it" },
                    flowSha?.let { "Flow SHA-256 · $it" },
                    lastStep?.let { "Last recorded step · $it" },
                    presentation?.activityDetail?.let { "Model slot · $it" },
                ).joinToString("\n")
                SelectionContainer { Text(identifiers, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
                presentation?.details?.let { details ->
                    SelectionContainer { Text(details, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
                }
                pollingError?.let { OperationError(it) }
                SectionHeading("Technical events")
                if (olderEventsOmitted) Text("Older technical events omitted from this view.")
                if (lines.isEmpty()) {
                    Text("No technical events are available.", style = MaterialTheme.typography.bodyMedium)
                } else {
                    TextButton(onClick = {
                        val latest = lines.lastIndex
                        if (latest >= 0) scope.launch { listState.scrollToItem(latest) }
                    }) {
                        Text("Jump to latest")
                    }
                    TextButton(onClick = {
                        val events = lines.joinToString("\n")
                        scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("Technical events", events))) }
                    }) { Text("Copy technical events") }
                    LazyColumn(
                        state = listState,
                        modifier = Modifier.fillMaxWidth().heightIn(max = 240.dp),
                    ) {
                        items(lines, key = { it }) { line ->
                            SelectionContainer { Text(line, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
                        }
                    }
                }
            }
            if (identity?.source == true) {
                DisclosureRow("Moray native report", showSourceReport) { showSourceReport = !showSourceReport }
                if (showSourceReport) SourceRunReportPanel(runtime, runId, presentation?.canRetainSourceReport == true)
            }
        }

        if (!notificationsEnabled && status?.workerActive == true) NotificationNotice()
        error?.let { OperationError(it) }
        startedRunError?.let { OperationError(it) }
    }
}

@Composable
private fun NotificationNotice() {
    Text("Background progress notifications are disabled. Enable them in Android app settings.", style = MaterialTheme.typography.bodySmall)
}
