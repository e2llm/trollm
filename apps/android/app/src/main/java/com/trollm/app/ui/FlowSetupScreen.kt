package com.trollm.app.ui

import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
import androidx.compose.runtime.DisposableEffect
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
import com.trollm.app.FlowSetup
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
import uniffi.trollm_uniffi.TrollmRuntime

/** Preparation does not start inference or access connection credentials. */
@Composable
fun FlowSetupScreen(
    runtime: TrollmRuntime,
    flowSha: String,
    onReady: (configurationJson: String) -> Unit,
    onBack: () -> Unit,
    sourceInstallation: String? = null,
) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val coordinator = remember(context.applicationContext) { ModelDownloadCoordinator.get(context) }
    val scope = rememberCoroutineScope()
    val overrideStore = remember(context.applicationContext) { flowSetupOverrideStore(context) }
    var defaults by remember(flowSha) { mutableStateOf<FlowSetup?>(null) }
    var settingsError by remember(flowSha) { mutableStateOf<String?>(null) }
    var settingsValid by remember(flowSha) { mutableStateOf(false) }
    var settingsRevision by remember(flowSha) { mutableStateOf(0) }
    var inspecting by remember(flowSha) { mutableStateOf(false) }
    var setup by remember(flowSha) { mutableStateOf<FlowSetup?>(null) }
    var states by remember(flowSha) { mutableStateOf<Map<String, ModelDownloadCoordinator.State>>(emptyMap()) }
    var error by remember(flowSha) { mutableStateOf<String?>(null) }
    var checking by remember(flowSha) { mutableStateOf<String?>(null) }
    var preparing by remember(flowSha) { mutableStateOf(false) }
    var pendingSlots by remember(flowSha) { mutableStateOf<List<String>>(emptyList()) }
    var batchSlot by remember(flowSha) { mutableStateOf<String?>(null) }
    var allowMetered by remember(flowSha) { mutableStateOf(false) }
    var busy by remember(flowSha) { mutableStateOf(false) }
    var cancelTarget by remember(flowSha) { mutableStateOf<Pair<String, ModelRequirement>?>(null) }
    var refresh by remember(flowSha) { mutableStateOf(0) }

    LaunchedEffect(runtime, flowSha) {
        try {
            val json = withContext(Dispatchers.IO) { if (sourceInstallation == null) runtime.flowSetup(flowSha) else runtime.sourceBundleSetup(sourceInstallation) }
            val pinned = json?.let { FlowSetup.parse(it) }
            defaults = pinned
            if (pinned != null) {
                try {
                    setup = withContext(Dispatchers.IO) { overrideStore.load(flowSha, pinned) }
                    settingsValid = true
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    setup = pinned
                    settingsError = "Saved advanced settings are invalid. Save corrected settings or restore defaults."
                }
            }
            if (json == null) error = "This flow has no model setup defaults. Existing model configuration is unchanged."
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            error = "Could not load this flow's setup."
        }
    }

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
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            // UI-scoped observation only; Android owns transfers while stopped.
            while (isActive) {
                try {
                    inspecting = true
                    states = coordinator.inspect(descriptor) { slot ->
                        withContext(Dispatchers.Main) { checking = slot }
                    }
                    checking = null
                    error = null
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: ModelPreparationException) {
                    checking = null
                    error = failure.message
                } catch (_: Exception) {
                    checking = null
                    error = "Could not inspect model storage."
                } finally { inspecting = false }
                delay(1500)
            }
        }
    }

    LaunchedEffect(preparing, pendingSlots, states, setup, busy) {
        val descriptor = setup ?: return@LaunchedEffect
        if (!preparing || busy || !settingsValid || states.size != descriptor.models.size) return@LaunchedEffect
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
        val selectedModel = model.copy(label = defaults?.let { descriptor.modelDisplayLabel(next, it) } ?: model.label)
        // Screen ownership survives effect restarts; leaving the screen may
        // cancel admission, while Android retains any already-enqueued request.
        scope.launch {
            try {
                coordinator.download(selectedModel, policy)
                states = states + (next to ModelDownloadCoordinator.State(downloading = true, total = model.sizeBytes))
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

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        ScreenHeader("Prepare models", onBack, setup?.title)
        setup?.description?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        Text("Download the models this flow uses. They stay saved on your device.")
        val downloadsIdle = !busy && !preparing && !inspecting && checking == null && states.values.none { it.downloading }
        ContentCard {
            SectionHeading(if (allowMetered) "Metered downloads allowed" else "Unmetered network only")
            Text("Downloads continue in the background. Keep this screen open to start the next model in a batch.", style = MaterialTheme.typography.bodyMedium)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(checked = allowMetered, onCheckedChange = { allowMetered = it }, enabled = downloadsIdle)
                Text("Allow metered data for downloads from this screen", style = MaterialTheme.typography.bodyMedium)
            }
            Text(ModelDownloadPolicy(allowMetered).description, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text("Additional free space is needed to finish setup.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        val remaining = setup?.let { descriptor ->
            remainingModelDownloads(descriptor.models, states.filterValues { it.path != null }.keys)
        }.orEmpty()
        SectionHeading("Models", "${states.values.count { it.path != null }} of ${setup?.models?.size ?: 0} ready")
        setup?.models?.forEach { (slot, model) ->
            val state = states[slot]
            val label = defaults?.let { pinned -> setup?.modelDisplayLabel(slot, pinned) } ?: slot
            val verifying = checking == slot
            ContentCard {
                Text(label, style = MaterialTheme.typography.titleMedium)
                StatusBadge(
                    when {
                        verifying -> "Verifying"
                        state?.path != null -> "Ready"
                        state?.downloading == true -> "Downloading"
                        state?.message != null -> "Needs attention"
                        else -> "Not downloaded"
                    },
                    when {
                        verifying -> StatusTone.INFO
                        state?.path != null -> StatusTone.SUCCESS
                        state?.message != null && !state.downloading -> StatusTone.WARNING
                        else -> StatusTone.NEUTRAL
                    },
                )
                Text(modelByteSize(model.sizeBytes), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                when {
                    verifying -> {
                        Text("Checking the downloaded file before use.", style = MaterialTheme.typography.bodyMedium)
                        LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                    }
                    state?.path != null -> Unit
                    state?.downloading == true -> {
                        val fraction = modelDownloadFraction(state.downloaded, state.total)
                        val percent = modelDownloadPercent(state.downloaded, state.total)
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            percent?.let { Text("$it%", style = MaterialTheme.typography.titleSmall) }
                            Text(
                                if (state.total > 0) "${modelByteSize(state.downloaded)} / ${modelByteSize(state.total)}"
                                else "${modelByteSize(state.downloaded)} downloaded",
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                        if (fraction == null) LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                        else LinearProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth())
                        state.message?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        TextButton(colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error), enabled = !busy && checking == null, onClick = { cancelTarget = slot to model }) { Text("Cancel download") }
                    }
                    else -> {
                        state?.message?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                        Button(shape = MaterialTheme.shapes.medium, enabled = settingsValid && downloadsIdle, onClick = {
                            scope.launch {
                                busy = true
                                try {
                                    coordinator.download(model.copy(label = label), ModelDownloadPolicy(allowMetered))
                                    states = states + (slot to ModelDownloadCoordinator.State(downloading = true, total = model.sizeBytes))
                                    refresh += 1
                                }
                                catch (cancelled: CancellationException) { throw cancelled }
                                catch (failure: ModelPreparationException) { error = failure.message ?: "Could not start download." }
                                catch (_: Exception) { error = "Could not start download." }
                                finally { busy = false }
                            }
                        }) { Text(if (state?.message != null) "Retry download" else "Download model") }
                    }
                }
            }
        }
        if (remaining.isNotEmpty()) {
            Text("${remaining.size} remaining models · ${modelByteSize(modelDownloadTotal(remaining.values.map { it.sizeBytes }))}", style = MaterialTheme.typography.bodyMedium)
            OutlinedButton(shape = MaterialTheme.shapes.medium, modifier = Modifier.fillMaxWidth(), enabled = settingsValid && downloadsIdle && states.keys == setup?.models?.keys, onClick = {
                pendingSlots = remaining.keys.toList()
                batchSlot = null
                preparing = true
                error = null
            }) { Text("Download remaining models") }
        }
        setup?.connections?.takeIf { it.isNotEmpty() }?.let { connections ->
            ContentCard {
                SectionHeading("Connections")
                connections.forEach { (name, connection) -> Text("$name · ${connection.auth}", style = MaterialTheme.typography.bodyMedium) }
            }
        }
        val pinned = defaults
        val effective = setup
        if (pinned != null && effective != null) {
            FlowSetupAdvancedSettings(
                defaults = pinned,
                effective = effective,
                resetKey = settingsRevision,
                enabled = !busy && !inspecting && checking == null && !preparing &&
                    states.keys == effective.models.keys && states.values.none { it.downloading },
                onSave = { updated ->
                    busy = true
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { overrideStore.save(flowSha, pinned, updated) }
                            preparing = false
                            states = emptyMap()
                            setup = updated
                            settingsValid = true
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
                            settingsValid = true
                            settingsRevision += 1
                            settingsError = null
                            refresh += 1
                        } catch (cancelled: CancellationException) { throw cancelled }
                        catch (_: Exception) { settingsError = "Could not restore defaults." }
                        finally { busy = false }
                    }
                },
            )
        }
        settingsError?.let { StatusPanel("Check advanced settings", it, StatusTone.ERROR) }
        error?.let { StatusPanel("Model preparation", it, StatusTone.ERROR) }
        val descriptor = setup
        val ready = settingsValid && descriptor != null && states.keys == descriptor.models.keys && states.values.all { it.path != null }
        Button(shape = MaterialTheme.shapes.medium, modifier = Modifier.fillMaxWidth(), enabled = ready && !busy && !inspecting && checking == null, onClick = {
            descriptor?.let { onReady(it.configuration(states.mapValues { entry -> requireNotNull(entry.value.path) })) }
        }) { Text("Continue") }
        if (!ready) Text("Finish downloading and verifying all models to continue.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
