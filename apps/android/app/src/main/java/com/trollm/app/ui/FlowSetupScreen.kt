package com.trollm.app.ui

import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
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
    var refresh by remember(flowSha) { mutableStateOf(0) }

    LaunchedEffect(runtime, flowSha) {
        try {
            val json = withContext(Dispatchers.IO) { runtime.flowSetup(flowSha) }
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
                        withContext(Dispatchers.Main) { checking = defaults?.let { descriptor.modelDisplayLabel(slot, it) } ?: "Model" }
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

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp)) {
        Text(setup?.title ?: "Prepare flow", style = MaterialTheme.typography.headlineSmall)
        setup?.description?.let { Text(it) }
        Text("Downloads continue in the background. Additional free space is needed to finish setup.")
        val downloadsIdle = !busy && !preparing && !inspecting && checking == null && states.values.none { it.downloading }
        Row {
            Checkbox(checked = allowMetered, onCheckedChange = { allowMetered = it }, enabled = downloadsIdle)
            Text("Allow metered data for downloads from this screen")
        }
        Text(ModelDownloadPolicy(allowMetered).description)
        val remaining = setup?.let { descriptor ->
            remainingModelDownloads(descriptor.models, states.filterValues { it.path != null }.keys)
        }.orEmpty()
        if (remaining.isNotEmpty()) {
            Text("${remaining.size} remaining models · ${remaining.values.sumOf { it.sizeBytes } / (1024 * 1024)} MiB")
            Button(enabled = settingsValid && downloadsIdle && states.keys == setup?.models?.keys, onClick = {
                pendingSlots = remaining.keys.toList()
                batchSlot = null
                preparing = true
                error = null
            }) { Text("Download remaining models") }
        }
        checking?.let { Text("Verifying $it…") }
        setup?.models?.forEach { (slot, model) ->
            val state = states[slot]
            val label = defaults?.let { pinned -> setup?.modelDisplayLabel(slot, pinned) } ?: slot
            Text(label, style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 16.dp))
            Text("${model.sizeBytes / (1024 * 1024)} MiB · context ${model.ctx}")
            when {
                state?.path != null -> Text("Ready")
                state?.downloading == true -> {
                    Text("${state.downloaded / (1024 * 1024)} / ${state.total / (1024 * 1024)} MiB · ${state.message ?: "Downloading"}")
                    LinearProgressIndicator(progress = { (state.downloaded.toFloat() / state.total.coerceAtLeast(1)).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
                    Button(enabled = !busy && checking == null, onClick = {
                        preparing = false
                        pendingSlots = emptyList()
                        scope.launch {
                            busy = true
                            try { coordinator.cancel(model); refresh += 1 }
                            catch (cancelled: CancellationException) { throw cancelled }
                            catch (_: Exception) { error = "Could not cancel the download." }
                            finally { busy = false }
                        }
                    }) { Text("Cancel download") }
                }
                else -> {
                    Text(state?.message ?: "Not downloaded")
                    Button(enabled = settingsValid && downloadsIdle, onClick = {
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
        setup?.connections?.forEach { (name, connection) -> Text("Connection: $name (${connection.auth})") }
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
        settingsError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        val descriptor = setup
        val ready = settingsValid && descriptor != null && states.keys == descriptor.models.keys && states.values.all { it.path != null }
        Button(enabled = ready && !busy && !inspecting && checking == null, onClick = {
            descriptor?.let { onReady(it.configuration(states.mapValues { entry -> requireNotNull(entry.value.path) })) }
        }) { Text("Continue") }
        Button(onClick = onBack) { Text("Back") }
    }
}
