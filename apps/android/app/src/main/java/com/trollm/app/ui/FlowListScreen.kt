package com.trollm.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import com.trollm.app.FlowSetup
import com.trollm.app.StarterFlows
import uniffi.trollm_uniffi.FlowInfo
import uniffi.trollm_uniffi.TrollmRuntime

/** Installed flows from `list_flows` (§9), content-addressed by sha256. */
@Composable
fun FlowListScreen(
    runtime: TrollmRuntime,
    runIds: List<String>,
    onOpenRun: (String) -> Unit,
    onRunFlow: (FlowInfo) -> Unit,
    onInstall: () -> Unit,
    onConnections: () -> Unit,
) {
    val context = LocalContext.current
    var flows by remember { mutableStateOf<List<FlowInfo>>(emptyList()) }
    var titles by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var error by remember { mutableStateOf<String?>(null) }

    androidx.compose.runtime.LaunchedEffect(Unit) {
        val starterError = runCatching { StarterFlows.ensureInstalled(context, runtime) }.exceptionOrNull()
        val listingError = runCatching {
            val listed = withContext(Dispatchers.IO) {
                runtime.listFlows().map { flow ->
                    val title = runCatching {
                        runtime.flowSetup(flow.sha256)?.let { FlowSetup.parse(it).title }
                    }.getOrNull() ?: flow.id
                    flow to title
                }
            }
            flows = listed.map { it.first }
            titles = listed.associate { it.first.sha256 to it.second }
        }.exceptionOrNull()
        error = (listingError ?: starterError)?.message
    }

    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text("trollm", style = MaterialTheme.typography.headlineSmall)
            Button(onClick = onInstall) { Text("Install bundle") }
        }
        Button(onClick = onConnections) { Text("Connections") }
        error?.let { Text("Error: $it", color = MaterialTheme.colorScheme.error) }
        LazyColumn(Modifier.fillMaxSize().padding(top = 16.dp)) {
            item { Text("Installed flows", style = MaterialTheme.typography.titleMedium) }
            items(flows) { flow ->
                Card(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .clickable { onRunFlow(flow) },
                ) {
                    Column(Modifier.padding(12.dp)) {
                        Text(titles[flow.sha256] ?: flow.id, style = MaterialTheme.typography.titleMedium)
                        Text(
                            "v${flow.version} · sha256 ${flow.sha256.take(12)}…",
                            fontFamily = FontFamily.Monospace,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
            if (runIds.isNotEmpty()) {
                item { Text("Run history", style = MaterialTheme.typography.titleMedium) }
                items(runIds) { id ->
                    Card(Modifier.fillMaxWidth().padding(vertical = 4.dp).clickable { onOpenRun(id) }) {
                        Text(id, modifier = Modifier.padding(12.dp), fontFamily = FontFamily.Monospace)
                    }
                }
            }
        }
    }
}
