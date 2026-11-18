package com.trollm.app.ui

import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.TextButton
import androidx.compose.material3.Checkbox
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import com.trollm.app.ModelRequirement
import com.trollm.app.StatusTone
import com.trollm.app.modelByteSize
import com.trollm.app.modelDownloadFraction
import com.trollm.app.modelDownloadPercent
import com.trollm.app.modelDownloadTotal
import com.trollm.app.BundledStarter
import com.trollm.app.FlowSetup
import com.trollm.app.ModelTransferPhase
import com.trollm.app.SecretsStore
import com.trollm.app.SelectedAuthMode
import com.trollm.app.catchingOperation
import com.trollm.app.modelNextFileFreeSpace
import com.trollm.app.flowSetupOverrideStore
import com.trollm.app.modelDisplayLabel
import com.trollm.app.ModelPreparationException
import com.trollm.app.ModelDownloadCoordinator
import com.trollm.app.ModelDownloadPolicy
import com.trollm.app.remainingModelDownloads
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.FlowInfo

/** Inspect saved configuration and transfer files only after an explicit download choice. */
@Composable
internal fun FlowSetupScreen(
    pinned: FlowSetup,
    flow: FlowInfo,
    sourceInstallation: String?,
    starter: BundledStarter?,
    starting: Boolean,
    onStart: (configurationJson: String) -> Unit,
    onBack: (() -> Unit)?,
    onOpenConnection: (String, String) -> Unit,
    inputs: @Composable ColumnScope.() -> Unit,
) {
    val flowSha = sourceInstallation ?: flow.sha256
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val lifecycleState by lifecycle.currentStateFlow.collectAsState()
    val coordinator by remember(context.applicationContext) { lazy { ModelDownloadCoordinator.get(context) } }
    val scope = rememberCoroutineScope()
    val overrideStore = remember(context.applicationContext) { flowSetupOverrideStore(context) }
    var settingsError by remember(flowSha) { mutableStateOf<String?>(null) }
    var settingsRevision by remember(flowSha) { mutableStateOf(0) }
    var inspecting by remember(flowSha) { mutableStateOf(false) }
    var setup by remember(flowSha) { mutableStateOf<FlowSetup?>(null) }
    var states by remember(flowSha) { mutableStateOf<Map<String, ModelDownloadCoordinator.State>>(emptyMap()) }
    var inspectionError by remember(flowSha) { mutableStateOf<String?>(null) }
    var error by remember(flowSha) { mutableStateOf<String?>(null) }
    var checking by remember(flowSha) { mutableStateOf<String?>(null) }
    var preparing by remember(flowSha) { mutableStateOf(false) }
    var pendingSlots by remember(flowSha) { mutableStateOf<List<String>>(emptyList()) }
    var batchSlot by remember(flowSha) { mutableStateOf<String?>(null) }
    var allowMetered by rememberSaveable(flowSha) { mutableStateOf(false) }
    var advanced by rememberSaveable(flowSha) { mutableStateOf(false) }
    var busy by remember(flowSha) { mutableStateOf(false) }
    var cancelTarget by remember(flowSha) { mutableStateOf<Pair<String, ModelRequirement>?>(null) }
    var refresh by remember(flowSha) { mutableStateOf(0) }
    val inputScroll = rememberScrollState()

    val closeAdvanced = { advanced = false }
    BackHandler(enabled = advanced && !starting, onBack = closeAdvanced)

    LaunchedEffect(flowSha, pinned) {
        catchingOperation {
            withContext(Dispatchers.IO) { overrideStore.load(flowSha, pinned) }
        }.onSuccess {
            setup = it
        }.onFailure {
            setup = pinned
            settingsError = "Saved advanced settings are invalid. Save corrected settings or restore defaults."
        }
    }

    val connections = setup?.connections.orEmpty()
    var savedConnections by remember(connections) { mutableStateOf<Map<String, SavedSignIn>>(emptyMap()) }
    var signInCheck by remember(flowSha) { mutableStateOf(0) }
    LaunchedEffect(connections, lifecycle, signInCheck) {
        if (connections.isEmpty()) return@LaunchedEffect
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            savedConnections = emptyMap()
            savedConnections = withContext(Dispatchers.IO) {
                connections.mapValues { (name, connection) ->
                    catchingOperation {
                        val secrets = SecretsStore(context.applicationContext)
                        val saved = when (secrets.selectedAuthMode(name)) {
                            SelectedAuthMode.BEARER -> secrets.token(name, connection.url) != null
                            SelectedAuthMode.OAUTH -> secrets.oauthGrants.pin(name, connection.url) != null
                            else -> false
                        }
                        if (saved) SavedSignIn.SAVED else SavedSignIn.MISSING
                    }.getOrDefault(SavedSignIn.UNREADABLE)
                }
            }
        }
    }

    val connectionBlocker = connections.entries.firstOrNull { savedConnections[it.key] != SavedSignIn.SAVED }
        ?.takeIf { starter == BundledStarter.BROWSER_SUMMARY }

    DisposableEffect(context, flowSha) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) {
                if (intent?.action == DownloadManager.ACTION_DOWNLOAD_COMPLETE) refresh += 1
            }
        }
        // DownloadManager may send from a privileged UID other than system UID.
        // The broadcast is a refresh hint only; all IDs/status come from our store.
        ContextCompat.registerReceiver(context, receiver, IntentFilter(DownloadManager.ACTION_DOWNLOAD_COMPLETE), ContextCompat.RECEIVER_EXPORTED)
        onDispose { context.unregisterReceiver(receiver) }
    }

    LaunchedEffect(setup, lifecycle, refresh) {
        val descriptor = setup ?: return@LaunchedEffect
        if (descriptor.models.isEmpty()) {
            states = emptyMap()
            checking = null
            inspecting = false
            inspectionError = null
            return@LaunchedEffect
        }
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            // UI-scoped observation only; Android owns transfers while stopped.
            while (isActive) {
                try {
                    inspecting = true
                    states = coordinator.inspect(descriptor) { slot ->
                        withContext(Dispatchers.Main) { checking = slot }
                    }
                    checking = null
                    inspectionError = null
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: ModelPreparationException) {
                    checking = null
                    inspectionError = failure.message
                } catch (_: Exception) {
                    checking = null
                    inspectionError = "Could not inspect model storage."
                } finally { inspecting = false }
                delay(1500)
            }
        }
    }

    LaunchedEffect(preparing, pendingSlots, states, setup, busy, lifecycleState, inspectionError, connectionBlocker) {
        val descriptor = setup ?: return@LaunchedEffect
        if (!preparing || busy || settingsError != null || inspectionError != null ||
            !lifecycleState.isAtLeast(Lifecycle.State.STARTED) || states.keys != descriptor.models.keys) return@LaunchedEffect
        if (connectionBlocker != null) {
            preparing = false
            return@LaunchedEffect
        }
        if (states.values.any { it.downloading }) return@LaunchedEffect
        // A failed transfer stops the explicit batch rather than silently retrying.
        if (batchSlot?.let { states[it]?.path == null } == true) {
            preparing = false
            return@LaunchedEffect
        }
        val next = pendingSlots.firstOrNull { states[it]?.path == null }
        if (next == null) {
            preparing = false
            return@LaunchedEffect
        }
        val model = descriptor.models.getValue(next)
        // Claim before suspension. Observation can restart this effect after
        // Android enqueues the transfer, but must never admit the slot twice.
        busy = true
        batchSlot = next
        pendingSlots = pendingSlots.dropWhile { it != next }.drop(1)
        val policy = ModelDownloadPolicy(allowMetered)
        val selectedModel = model.copy(label = descriptor.modelDisplayLabel(next, pinned))
        // Screen ownership survives effect restarts; leaving the screen may
        // cancel admission, while Android retains any already-enqueued request.
        scope.launch {
            try {
                coordinator.download(selectedModel, policy)
                states = states + (next to ModelDownloadCoordinator.State(downloading = true, total = model.sizeBytes, transferPhase = ModelTransferPhase.QUEUED))
                refresh += 1
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: ModelPreparationException) {
                error = failure.message ?: "Could not start the model download."
                preparing = false
            } catch (_: Exception) {
                error = "Could not start the model download."
                preparing = false
            } finally { busy = false }
        }
    }

    val effective = setup
    val inspected = effective != null && states.keys == effective.models.keys && inspectionError == null
    val remaining = effective?.let { descriptor ->
        remainingModelDownloads(descriptor.models, states.filterValues { it.path != null }.keys)
    }.orEmpty()
    val allSlotsReady = inspected && states.values.all { it.path != null }
    val activeTransfer = states.entries.firstOrNull { it.value.downloading }
    val downloadsIdle = inspected && !starting && !busy && !preparing && !inspecting && checking == null && activeTransfer == null
    val stockBrowser = starter == BundledStarter.BROWSER_SUMMARY
    val hasOverrides = effective != null && (connections.isNotEmpty() || effective.models.isNotEmpty() || settingsError != null)
    val openAdvanced = { advanced = true }
    val queueDownloads: (List<String>) -> Unit = { slots ->
        pendingSlots = slots
        batchSlot = null
        preparing = true
        error = null
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(if (advanced) rememberScrollState() else inputScroll).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(if (advanced) 16.dp else 24.dp),
    ) {
        if (advanced && effective != null) {
            ScreenHeader("Setup details", onBack = closeAdvanced.takeUnless { starting }, subtitle = pinned.title)
            SelectionContainer {
                Text(
                    "Flow ID · ${flow.id}\nVersion · ${flow.version}\nFlow SHA-256 · ${flow.sha256}",
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            effective.models.forEach { (slot, model) ->
                val state = states[slot]
                SectionHeading(effective.modelDisplayLabel(slot, pinned))
                Text(modelByteSize(model.sizeBytes), style = MaterialTheme.typography.bodyMedium)
                when {
                    checking == slot -> Text("Verifying file")
                    state?.path != null -> Text("Verified and saved")
                    state?.downloading == true -> {
                        ModelTransfer(state)
                        CancelDownloadButton(enabled = !busy && checking == null) { cancelTarget = slot to model }
                    }
                    !inspected -> Text("Checking saved files")
                    else -> {
                        state?.message?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                        OutlinedButton(
                            onClick = { queueDownloads(listOf(slot)) },
                            enabled = downloadsIdle && settingsError == null && connectionBlocker == null,
                        ) { Text(if (state?.message != null) "Retry download" else "Download model") }
                    }
                }
            }
            connections.forEach { (name, connection) ->
                if (savedConnections[name] == SavedSignIn.UNREADABLE) {
                    Text("$name · Could not read saved sign-in", style = MaterialTheme.typography.bodyMedium)
                } else {
                    TextButton(enabled = !starting, onClick = { onOpenConnection(name, connection.url) }) {
                        Text("Set up $name connection")
                    }
                }
            }
            if (hasOverrides) FlowSetupAdvancedSettings(
                defaults = pinned,
                effective = effective,
                resetKey = settingsRevision,
                enabled = !starting && !busy && !inspecting && checking == null && !preparing &&
                    activeTransfer == null && (inspected || inspectionError != null),
                onSave = { updated ->
                    busy = true
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { overrideStore.save(flowSha, pinned, updated) }
                            preparing = false
                            states = emptyMap()
                            setup = updated
                            settingsRevision += 1
                            settingsError = null
                            refresh += 1
                        } catch (cancelled: CancellationException) { throw cancelled }
                        catch (_: Exception) { settingsError = "Could not save advanced settings." }
                        finally { busy = false }
                    }
                },
                onRestore = {
                    busy = true
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { overrideStore.restore(flowSha) }
                            preparing = false
                            states = emptyMap()
                            setup = pinned
                            settingsRevision += 1
                            settingsError = null
                            refresh += 1
                        } catch (cancelled: CancellationException) { throw cancelled }
                        catch (_: Exception) { settingsError = "Could not restore defaults." }
                        finally { busy = false }
                    }
                },
            )
        } else {
            ScreenHeader(pinned.title, onBack = onBack, subtitle = pinned.description.takeIf { starter == null })
            inputs()
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                SectionHeading("Before you run")
                if (stockBrowser) {
                    Text(
                        "Have your E2LLM browser connected and its name or ID ready. Browser availability is checked during the run.",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                connections.forEach { (name, connection) ->
                    val serviceName = if (stockBrowser) "E2LLM" else name
                    val saved = savedConnections[name]
                    if (connectionBlocker?.key != name) {
                        Text(when (saved) {
                            SavedSignIn.SAVED -> "$serviceName · Sign-in saved"
                            SavedSignIn.MISSING -> "$serviceName · ${signInMethod(connection.auth)} setup available"
                            SavedSignIn.UNREADABLE -> "$serviceName · Could not read saved sign-in"
                            null -> "$serviceName · Checking saved sign-in"
                        }, style = MaterialTheme.typography.bodyMedium)
                        if (saved == SavedSignIn.MISSING) TextButton(enabled = !starting, onClick = { onOpenConnection(name, connection.url) }) {
                            Text("Set up $name connection")
                        }
                    }
                }
                when {
                    settingsError != null -> {
                        StatusPanel("Check advanced settings", settingsError, StatusTone.ERROR)
                        Button(onClick = openAdvanced, enabled = !starting, modifier = Modifier.fillMaxWidth()) { Text("Review setup details") }
                    }
                    checking != null -> StatusPanel(
                        "Verifying model file", "Checking the file before use.", StatusTone.INFO,
                        Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                    )
                    activeTransfer != null -> {
                        val (slot, state) = activeTransfer
                        val model = effective?.models?.get(slot)
                        ModelTransfer(state)
                        if (model != null) CancelDownloadButton(enabled = !busy && checking == null) {
                            cancelTarget = slot to model
                        }
                    }
                    connectionBlocker != null -> {
                        val (name, connection) = connectionBlocker
                        val serviceName = if (stockBrowser) "E2LLM" else name
                        val liveStatus = Modifier.semantics { liveRegion = LiveRegionMode.Polite }
                        when (savedConnections[name]) {
                            null -> StatusPanel("Checking saved sign-in", tone = StatusTone.INFO, modifier = liveStatus)
                            SavedSignIn.UNREADABLE -> {
                                StatusPanel("Could not read saved sign-in for $serviceName",
                                    tone = StatusTone.ERROR, modifier = liveStatus)
                                Button(
                                    onClick = { signInCheck++ },
                                    enabled = !starting,
                                    modifier = Modifier.fillMaxWidth(),
                                ) { Text("Check saved sign-in again") }
                            }
                            SavedSignIn.MISSING -> {
                                StatusPanel("Sign in to $serviceName", signInMethod(connection.auth),
                                    StatusTone.WARNING, liveStatus)
                                Button(
                                    onClick = { onOpenConnection(name, connection.url) },
                                    enabled = !starting,
                                    modifier = Modifier.fillMaxWidth(),
                                ) { Text("Set up $serviceName connection") }
                            }
                            SavedSignIn.SAVED -> Unit
                        }
                    }
                    inspectionError != null -> StatusPanel(
                        "Model files could not be checked", inspectionError, StatusTone.ERROR,
                        Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                    )
                    !inspected || remaining.isEmpty() && !allSlotsReady -> StatusPanel("Checking saved files", tone = StatusTone.INFO,
                        modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                    remaining.isNotEmpty() -> {
                        val total = modelByteSize(modelDownloadTotal(remaining.values.map { it.sizeBytes }))
                        val freeSpace = modelByteSize(modelNextFileFreeSpace(remaining.values.first().sizeBytes))
                        val failedTransfer = remaining.keys.firstNotNullOfOrNull { states[it]?.message }
                        StatusPanel(
                            title = if (preparing || busy) "Queuing model download" else if (remaining.size == 1) "Model download needed" else "Model downloads needed",
                            message = "$total to download. The next file needs $freeSpace free, including 256 MiB headroom.",
                            tone = StatusTone.WARNING,
                            modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                        )
                        failedTransfer?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 48.dp).toggleable(
                                value = allowMetered, enabled = downloadsIdle, role = Role.Checkbox,
                                onValueChange = { allowMetered = it },
                            ),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Checkbox(checked = allowMetered, onCheckedChange = null, enabled = downloadsIdle)
                            Text("Allow metered data", style = MaterialTheme.typography.bodyMedium)
                        }
                        Text(ModelDownloadPolicy(allowMetered).description, style = MaterialTheme.typography.bodySmall)
                        if (remaining.size > 1) Text(
                            "Keep this screen open to queue the next model. A queued transfer can continue after you leave.",
                            style = MaterialTheme.typography.bodySmall,
                        )
                        Button(
                            onClick = { queueDownloads(remaining.keys.toList()) },
                            enabled = downloadsIdle,
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text(when {
                            remaining.size > 1 -> "Download remaining models · $total"
                            failedTransfer != null -> "Retry download · $total"
                            else -> "Download model · $total"
                        }) }
                    }
                    effective != null -> {
                        StatusPanel(
                            if (effective.models.isEmpty()) "No model files required" else "Models verified and saved",
                            tone = StatusTone.SUCCESS,
                            modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                        )
                        Button(
                            onClick = { onStart(effective.configuration(states.mapValues { requireNotNull(it.value.path) })) },
                            enabled = downloadsIdle,
                            modifier = Modifier.fillMaxWidth(),
                        ) { Text(if (starting) "Starting…" else "Start run") }
                    }
                }
                if (effective != null && settingsError == null) {
                    TextButton(onClick = openAdvanced, enabled = !starting) { Text("Setup details") }
                }
            }
        }
        if (advanced) settingsError?.let { StatusPanel("Check advanced settings", it, StatusTone.ERROR) }
        error?.let { StatusPanel("Model preparation", it, StatusTone.ERROR) }
    }

    cancelTarget?.let { (slot, model) ->
        val descriptor = setup
        val current = states[slot]
        val canCancel = !busy && !inspecting && checking == null && descriptor?.models?.get(slot) == model && current?.downloading == true && current.path == null
        AlertDialog(
            onDismissRequest = { cancelTarget = null },
            title = { Text("Cancel this download?") },
            text = { Text("Downloaded progress for this model will be discarded. Ready models stay saved. Any remaining models in this batch will stop queuing.") },
            dismissButton = { TextButton(onClick = { cancelTarget = null }) { Text("Keep downloading") } },
            confirmButton = {
                TextButton(colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error), enabled = canCancel, onClick = {
                    // Claim before suspension, then refresh Android state: a dialog can
                    // remain open while a transfer finishes or its model changes.
                    if (canCancel && descriptor != null) {
                        busy = true
                        cancelTarget = null
                        scope.launch {
                            try {
                                val latest = coordinator.inspect(descriptor) { checkingSlot -> withContext(Dispatchers.Main) { checking = checkingSlot } }
                                states = latest
                                checking = null
                                if (setup == descriptor && setup?.models?.get(slot) == model && latest[slot]?.downloading == true && latest[slot]?.path == null) {
                                    preparing = false
                                    pendingSlots = emptyList()
                                    coordinator.cancel(model)
                                    refresh += 1
                                }
                            } catch (cancelled: CancellationException) { throw cancelled }
                            catch (_: Exception) { error = "Could not cancel the download." }
                            finally { checking = null; busy = false }
                        }
                    }
                }) { Text("Cancel download") }
            },
        )
    }
}


private fun transferTitle(phase: ModelTransferPhase?): String = when (phase) {
    ModelTransferPhase.QUEUED -> "Download queued"
    ModelTransferPhase.WAITING_FOR_NETWORK -> "Waiting for network"
    ModelTransferPhase.PAUSED -> "Download paused"
    ModelTransferPhase.TRANSFERRING -> "Downloading model"
    null -> "Queuing model download"
}

@Composable
private fun ModelTransfer(state: ModelDownloadCoordinator.State) {
    StatusPanel(transferTitle(state.transferPhase), tone = StatusTone.INFO,
        modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
    state.message?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
    if (state.transferPhase != ModelTransferPhase.TRANSFERRING) return
    val fraction = modelDownloadFraction(state.downloaded, state.total)
    if (fraction == null) LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
    else {
        LinearProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth())
        val percent = requireNotNull(modelDownloadPercent(state.downloaded, state.total))
        Text(
            "${modelByteSize(state.downloaded)} of ${modelByteSize(state.total)} · $percent%",
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun CancelDownloadButton(enabled: Boolean, onClick: () -> Unit) {
    TextButton(
        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
        enabled = enabled,
        onClick = onClick,
    ) { Text("Cancel download") }
}


private fun signInMethod(auth: String): String =
    if (auth == "oauth") "OAuth sign-in" else "Manual Bearer token"

private enum class SavedSignIn { SAVED, MISSING, UNREADABLE }
