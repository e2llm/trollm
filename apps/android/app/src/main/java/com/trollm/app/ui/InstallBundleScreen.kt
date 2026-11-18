package com.trollm.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.trollm.app.BundleInput
import com.trollm.app.StatusTone
import com.trollm.app.catchingOperation
import com.trollm.app.installedFlow
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.InstallResult
import uniffi.trollm_uniffi.TrollmRuntime

/** Bounded file import; integrity verification does not establish author identity. */
@Composable
fun InstallBundleScreen(
    runtime: TrollmRuntime,
    onDone: () -> Unit,
    onOpen: ((FlowInfo) -> Unit)? = null,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var installed by remember { mutableStateOf<InstallResult?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocument(),
    ) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        busy = true
        installed = null
        failure = null
        scope.launch {
            val outcome = withContext(Dispatchers.IO) {
                catchingOperation {
                    val bytes = context.contentResolver.openInputStream(uri)?.use { BundleInput.read(it) }
                        ?: error("could not read selected file")
                    runtime.installFlowBundle(bytes)
                }
            }
            installed = outcome.getOrNull()
            failure = outcome.exceptionOrNull()?.let { it.message ?: "Could not read or install this file." }
            busy = false
        }
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        ScreenHeader(title = "Import flow file", onBack = onDone)
        val result = installed
        if (result == null) {
            Text(
                "Choose a .tar flow bundle. The app checks its files and flow definition before installing it.",
                style = MaterialTheme.typography.bodyLarge,
            )
            Text(
                "Only import flows from authors you trust. Flows can use granted service access to read, send or change data.",
                style = MaterialTheme.typography.bodyMedium,
            )
            Button(
                onClick = { picker.launch(arrayOf("*/*")) },
                enabled = !busy,
                modifier = Modifier.heightIn(min = 48.dp),
                shape = MaterialTheme.shapes.medium,
            ) { Text("Choose file") }
            if (busy) {
                StatusPanel(
                    "Verifying bundle…",
                    tone = StatusTone.INFO,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
            }
            failure?.let {
                StatusPanel(
                    "Import rejected",
                    it,
                    StatusTone.ERROR,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
                )
            }
        } else {
            StatusPanel(
                title = "Flow installed",
                message = "${result.flowId} · Version ${result.flowVersion}\n${result.filesVerified} files verified against the bundle manifest.",
                tone = StatusTone.SUCCESS,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
            if (onOpen != null) {
                Button(
                    onClick = { onOpen(result.installedFlow()) },
                    modifier = Modifier.heightIn(min = 48.dp),
                    shape = MaterialTheme.shapes.medium,
                ) { Text("Open this flow") }
            }
            Text(
                "File checksums confirm integrity against the manifest. Author identity is unverified.",
                style = MaterialTheme.typography.bodyMedium,
            )
            TechnicalDetails(
                "Signature status: ${result.signatureStatus}\nFlow SHA-256: ${result.flowSha256}\n" +
                    "Installation SHA-256: ${result.installationSha256}",
            )
            TextButton(
                onClick = { picker.launch(arrayOf("*/*")) },
                modifier = Modifier.heightIn(min = 48.dp),
            ) { Text("Choose another file") }
        }
    }
}
