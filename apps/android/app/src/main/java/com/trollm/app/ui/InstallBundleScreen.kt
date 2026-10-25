package com.trollm.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.InstallResult
import uniffi.trollm_uniffi.TrollmRuntime

/** Bounded file import; integrity verification does not establish author identity. */
@Composable
fun InstallBundleScreen(runtime: TrollmRuntime, onDone: () -> Unit) {
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
                runCatching {
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
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        ScreenHeader(
            title = "Install flow",
            onBack = onDone,
            subtitle = "Choose a flow bundle saved on your device.",
        )
        ContentCard {
            SectionHeading("Flow bundle")
            Text("Select a .tar bundle. Its files and flow definition are checked before installation.", style = MaterialTheme.typography.bodyMedium)
            Text("Only install flows from authors you trust. Flows can use granted service access to read, send or change data.", style = MaterialTheme.typography.bodyMedium)
            Button(
                onClick = { picker.launch(arrayOf("*/*")) },
                enabled = !busy,
                shape = MaterialTheme.shapes.medium,
            ) { Text(if (busy) "Installing…" else "Choose file") }
        }
        if (busy) {
            StatusPanel("Verifying bundle…", tone = StatusTone.INFO,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
        failure?.let {
            StatusPanel("Install rejected", it, StatusTone.ERROR,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
        installed?.let { result ->
            StatusPanel(
                title = "Flow installed",
                message = "${result.flowId} · Version ${result.flowVersion}\n${result.filesVerified} files verified against the bundle manifest.",
                tone = StatusTone.SUCCESS,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
            ContentCard {
                SectionHeading("Author verification")
                Text("Author identity is not verified. File checksums confirm integrity against the manifest; they do not verify who created the bundle.", style = MaterialTheme.typography.bodyMedium)
                Text("Signature status: ${result.signatureStatus}", style = MaterialTheme.typography.bodySmall)
                TechnicalDetails("Flow SHA-256: ${result.flowSha256}")
            }
        }
    }
}
