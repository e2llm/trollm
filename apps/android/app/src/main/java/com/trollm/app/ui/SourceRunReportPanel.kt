package com.trollm.app.ui

import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import com.trollm.app.MoraySource
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.trollm_uniffi.TrollmRuntime

/** Native observations enter durable custody before any explicit network send. */
@Composable
fun SourceRunReportPanel(runtime: TrollmRuntime, runId: String, terminal: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var source by remember(runId) { mutableStateOf(false) }
    var saved by remember(runId) { mutableStateOf<JSONObject?>(null) }
    var busy by remember(runId) { mutableStateOf(false) }
    var problem by remember(runId) { mutableStateOf<String?>(null) }
    LaunchedEffect(runId) {
        try {
            source = withContext(Dispatchers.IO) { runtime.isSourceRun(runId) }
            if (source) saved = withContext(Dispatchers.IO) { MoraySource.reports(context, runtime).retained(runId) }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { problem = "Retained source-report state could not be read." }
    }
    if (!source) { problem?.let { Text(it) }; return }
    ContentCard {
        SectionHeading("Moray native report")
        Text("Reports contain native progress observations. Business completion is not assessed; model prose is not verification.")
        saved?.let {
            Text("Report: ${it.getString("reportId")}")
            Text(if (it.has("receipt")) "Delivery acknowledged as device-reported." else "Retained locally; delivery not confirmed.")
        }
        Button(enabled = terminal && !busy, onClick = {
            busy = true; problem = null
            scope.launch {
                try { saved = withContext(Dispatchers.IO) { MoraySource.reports(context, runtime).retain(runId) } }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { problem = "Native report could not be retained. Execution is unchanged." }
                finally { busy = false }
            }
        }) { Text("Retain native observations") }
        Button(enabled = saved != null && saved?.has("receipt") == false && !busy, onClick = {
            busy = true; problem = null
            scope.launch {
                try { saved = withContext(Dispatchers.IO) { MoraySource.reports(context, runtime).send(runId) } }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { problem = "Report delivery not confirmed. Retry sends the same retained report without repeating execution." }
                finally { busy = false }
            }
        }) { Text("Send or retry retained report") }
        problem?.let { Text(it) }
    }
}
