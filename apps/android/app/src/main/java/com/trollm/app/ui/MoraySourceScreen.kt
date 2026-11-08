package com.trollm.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.trollm.app.MorayFailure
import com.trollm.app.MorayProfile
import com.trollm.app.MoraySource
import com.trollm.app.MorayMachineInstallation
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.runInterruptible
import org.json.JSONObject
import uniffi.trollm_uniffi.TrollmRuntime

/** Explicit known-publication selection; opening this screen starts no run. */
@Composable
fun MoraySourceScreen(runtime: TrollmRuntime, onOpen: (JSONObject) -> Unit, onBack: () -> Unit) {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("moray-source-profile", 0) }
    var api by remember { mutableStateOf(prefs.getString("api", "") ?: "") }
    var issuer by remember { mutableStateOf(prefs.getString("issuer", "") ?: "") }
    var client by remember { mutableStateOf(prefs.getString("client", "") ?: "") }
    var publication by remember { mutableStateOf(prefs.getString("publication", "") ?: "") }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    fun perform(work: suspend (MorayProfile) -> Unit) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                val profile = MorayProfile(api, issuer, client, com.trollm.app.BuildConfig.SOURCE_REVISION)
                withContext(Dispatchers.IO) {
                    check(prefs.edit().putString("api", api).putString("issuer", issuer)
                        .putString("client", client).putString("publication", publication).commit())
                }
                work(profile)
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (failure: MorayFailure) { message = failure.code }
            catch (_: Exception) { message = "Source operation failed. Check the installed machine profile." }
            finally { busy = false }
        }
    }
    Column(Modifier.padding(16.dp).verticalScroll(rememberScrollState())) {
        Text("Moray source")
        Text("Source access uses a provisioned machine identity, separate from model and browser credentials.")
        Text("Installation ID: ${MorayMachineInstallation.deviceId(context)}")
        OutlinedTextField(api, { api = it }, label = { Text("Publication API HTTPS URL") }, enabled = !busy)
        OutlinedTextField(issuer, { issuer = it }, label = { Text("OAuth issuer HTTPS URL") }, enabled = !busy)
        OutlinedTextField(client, { client = it }, label = { Text("Machine client ID") }, enabled = !busy)
        OutlinedTextField(publication, { publication = it }, label = { Text("Publication ID") }, enabled = !busy)
        Button(enabled = !busy, onClick = {
            busy = true
            scope.launch {
                try {
                    val installed = withContext(Dispatchers.IO) { MorayMachineInstallation.importPrivate(context) }
                    api = installed.api; issuer = installed.issuer; client = installed.clientId
                    check(prefs.edit().putString("api", api).putString("issuer", issuer).putString("client", client).commit())
                    message = "Machine identity provisioned. No publication or run was started."
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Exception) { message = "Private machine provisioning failed. Check the operator installation input." }
                finally { busy = false }
            }
        }) { Text("Import private machine provisioning") }
        Button(enabled = !busy, onClick = { perform {
            withContext(Dispatchers.IO) { MorayMachineInstallation.remove(context, it) }
            message = "Local machine credential removed. Issued tokens retain the server's expiry and revocation policy."
        } }) { Text("Remove local machine credential") }
        Button(enabled = !busy, onClick = { perform { profile ->
            val installed = runInterruptible(Dispatchers.IO) { MoraySource.install(context, profile, publication, runtime) }
            onOpen(installed)
        } }) { Text("Install selected publication") }
        message?.let { Text(it) }
        Button(onClick = onBack, enabled = !busy) { Text("Back") }
    }
}
