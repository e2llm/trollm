package com.trollm.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.trollm.app.BundleInput
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.trollm_uniffi.TrollmRuntime

/**
 * Sideload a flow bundle (uncompressed tar; enterprise channel / USB-ADB
 * path per §8). The Rust side verifies every file's sha256 against the
 * manifest and runs core validation — this screen displays that result
 * verbatim, including the honest signature status (tech-debt #1).
 */
@Composable
fun InstallBundleScreen(runtime: TrollmRuntime, onDone: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf("Pick a flow bundle (.tar) to install.") }
    var busy by remember { mutableStateOf(false) }

    val picker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocument(),
    ) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        busy = true
        status = "Verifying…"
        scope.launch {
            status = withContext(Dispatchers.IO) {
                runCatching {
                    val bytes = context.contentResolver.openInputStream(uri)?.use { BundleInput.read(it) }
                        ?: error("could not read selected file")
                    val result = runtime.installFlowBundle(bytes)
                    buildString {
                        appendLine("Installed: ${result.flowId} v${result.flowVersion}")
                        appendLine("sha256: ${result.flowSha256}")
                        appendLine("files verified: ${result.filesVerified}")
                        append("signatures: ${result.signatureStatus}")
                    }
                }.getOrElse { "Install rejected:\n${it.message}" }
            }
            busy = false
        }
    }

    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Text("Install flow bundle", style = MaterialTheme.typography.headlineSmall)
        Button(
            onClick = { picker.launch(arrayOf("*/*")) },
            enabled = !busy,
            modifier = Modifier.padding(vertical = 16.dp),
        ) { Text(if (busy) "Installing…" else "Choose file") }
        Text(
            status,
            fontFamily = FontFamily.Monospace,
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState()),
        )
        Button(onClick = onDone, modifier = Modifier.padding(top = 16.dp)) { Text("Back") }
    }
}
