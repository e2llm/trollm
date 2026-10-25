package com.trollm.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.trollm.app.OAuthProblemException
import com.trollm.app.ConnectionInput
import com.trollm.app.StatusTone
import com.trollm.app.SecretsStore
import com.trollm.app.OAuthConnections
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Browser OAuth and manual Bearer setup; saved secrets never enter UI state. */
@Composable
fun ConnectionsScreen(onBack: () -> Unit) {
    val context = LocalContext.current.applicationContext
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val scope = rememberCoroutineScope()
    var server by remember { mutableStateOf("e2llm") }
    var endpoint by remember { mutableStateOf("https://mcp.e2llm.com/mcp") }
    var token by remember { mutableStateOf("") }
    var saved by remember { mutableStateOf<Boolean?>(null) }
    var savedForEndpoint by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var removeTokenFor by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var store by remember { mutableStateOf<SecretsStore?>(null) }
    var oauth by remember { mutableStateOf<OAuthConnections?>(null) }
    var oauthStatus by remember { mutableStateOf("Opening connection storage…") }
    var refreshStatus by remember { mutableStateOf(0) }
    val endpointError = ConnectionInput.endpointError(endpoint)

    LaunchedEffect(Unit) {
        try {
            store = withContext(Dispatchers.IO) { SecretsStore(context) }
            oauth = withContext(Dispatchers.IO) { OAuthConnections.get(context) }
        } catch (error: kotlinx.coroutines.CancellationException) {
            throw error
        } catch (_: Exception) {
            message = "Could not open connection storage."
        }
    }

    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) token = ""
            if (event == Lifecycle.Event.ON_RESUME) refreshStatus++
        }
        lifecycle.addObserver(observer)
        onDispose {
            token = ""
            lifecycle.removeObserver(observer)
        }
    }

    LaunchedEffect(server, endpoint, store, oauth, busy, refreshStatus) {
        saved = null
        savedForEndpoint = false
        if (!ConnectionInput.validServer(server)) {
            oauthStatus = "Enter a connection name without whitespace."
            return@LaunchedEffect
        }
        if (endpointError != null) {
            oauthStatus = "Enter a valid HTTPS server address to check this connection."
            return@LaunchedEffect
        }
        oauthStatus = if (busy) "Updating connection…" else "Opening connection storage…"
        val secrets = store
        if (!busy && secrets != null && ConnectionInput.validServer(server)) {
            try {
                val status = withContext(Dispatchers.IO) {
                    secrets.hasToken(server) to (secrets.token(server, endpoint) != null)
                }
                saved = status.first
                savedForEndpoint = status.second
                oauthStatus = withContext(Dispatchers.IO) {
                    oauth?.status(server, endpoint) ?: "Opening connection storage…"
                }
            } catch (error: kotlinx.coroutines.CancellationException) {
                throw error
            } catch (failure: OAuthProblemException) {
                message = failure.problem.message
                oauthStatus = failure.problem.message
            } catch (_: Exception) {
                message = "Could not read connection status."
                oauthStatus = "Check the HTTPS server address and connection storage."
            }
        }
    }

    fun update(remove: Boolean, selectedServer: String = server) {
        if (busy) return
        val secrets = store ?: return
        if (!ConnectionInput.validServer(selectedServer)) {
            message = "Enter a connection name without whitespace."
            return
        }
        if (!remove && !ConnectionInput.validToken(token)) {
            message = "Enter a token without whitespace or the Bearer prefix."
            return
        }
        val url = if (remove) null else try {
            ConnectionInput.canonicalEndpoint(endpoint)
        } catch (_: IllegalArgumentException) {
            message = "Enter a valid HTTPS server address before saving a token."
            return
        }
        val name = selectedServer
        val value = if (remove) null else token
        token = ""
        busy = true
        message = null
        scope.launch {
            try {
                withContext(Dispatchers.IO) {
                    if (remove) secrets.removeToken(name) else secrets.setToken(name, url, value)
                }
                saved = !remove
                savedForEndpoint = !remove
                message = if (remove) "Manual token removed for $name." else "Token saved."
            } catch (error: kotlinx.coroutines.CancellationException) {
                throw error
            } catch (_: Exception) {
                saved = null
                message = "Could not update connection."
            } finally {
                busy = false
                refreshStatus++
            }
        }
    }

    removeTokenFor?.let { name ->
        AlertDialog(
            onDismissRequest = { removeTokenFor = null },
            title = { Text("Remove manual token?") },
            text = { Text("Remove the manual Bearer token stored on this device for connection \"$name\" and its saved endpoint binding. OAuth sign-in and other connections stay saved. This does not revoke the token at the remote service.") },
            confirmButton = {
                TextButton(enabled = !busy && store != null, colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error), onClick = {
                    removeTokenFor = null
                    update(true, name)
                }) { Text("Remove token") }
            },
            dismissButton = { TextButton(onClick = { removeTokenFor = null }) { Text("Keep token") } },
        )
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        ScreenHeader(
            title = "Connections",
            onBack = { token = ""; onBack() },
            subtitle = "Connect to the services your flows use.",
        )
        ContentCard {
            SectionHeading("Server")
            Text("Use the connection name from your flow.", style = MaterialTheme.typography.bodyMedium)
            OutlinedTextField(
                value = server,
                onValueChange = {
                    server = it; token = ""; message = null
                    saved = null; savedForEndpoint = false; oauthStatus = "Checking connection input…"
                },
                label = { Text("Connection name") },
                singleLine = true,
                enabled = !busy,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = endpoint,
                onValueChange = {
                    endpoint = it; message = null
                    saved = null; savedForEndpoint = false; oauthStatus = "Checking connection input…"
                },
                label = { Text("Server address") },
                isError = endpointError != null,
                supportingText = { Text(endpointError ?: "Use the HTTPS address of the MCP service.") },
                singleLine = true,
                enabled = !busy,
                modifier = Modifier.fillMaxWidth(),
            )
        }
        ContentCard {
            SectionHeading("Browser sign-in")
            Text("Sign in through your browser to authorize this connection.", style = MaterialTheme.typography.bodyMedium)
            StatusPanel(oauthStatus, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
            Button(onClick = {
                val client = oauth ?: return@Button
                if (!ConnectionInput.validServer(server)) {
                    message = "Enter a connection name without whitespace."
                    return@Button
                }
                val name = server
                val url = endpoint
                busy = true
                message = null
                scope.launch {
                    try {
                        client.signIn(name, url)
                    } catch (error: kotlinx.coroutines.CancellationException) {
                        throw error
                    } catch (failure: OAuthProblemException) {
                        message = failure.problem.message
                    } catch (_: Exception) {
                        message = "Could not start sign-in. Check the endpoint, trust configuration and browser."
                    } finally {
                        busy = false
                        refreshStatus++
                    }
                }
            }, shape = MaterialTheme.shapes.medium, enabled = !busy && oauth != null && endpointError == null && ConnectionInput.validServer(server)) { Text("Sign in with browser") }
            TextButton(onClick = {
                val client = oauth ?: return@TextButton
                val name = server
                busy = true
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { client.logout(name) }
                        message = "OAuth account signed out. Manual tokens are managed separately."
                    } catch (error: kotlinx.coroutines.CancellationException) {
                        throw error
                    } catch (failure: OAuthProblemException) { message = failure.problem.message }
                    catch (_: Exception) { message = "Could not sign out of this connection." }
                    finally {
                        busy = false
                        refreshStatus++
                    }
                }
            }, enabled = !busy && oauth != null && ConnectionInput.validServer(server)) { Text("Sign out of OAuth") }
        }
        ContentCard {
            SectionHeading("Manual token")
            Text("Saving a Bearer token switches this connection away from browser sign-in (OAuth).", style = MaterialTheme.typography.bodyMedium)
            Text(when (saved) {
                true -> if (savedForEndpoint) "Saved token available for this endpoint."
                    else "Token stored but inactive for this endpoint. Re-save it with its intended endpoint to use manual authentication."
                false -> "No saved token."
                null -> if (endpointError != null) "Enter a valid endpoint to check saved-token status."
                    else if (busy) "Updating saved-token status…"
                    else "Saved-token status is unavailable."
            })
            OutlinedTextField(
                value = token,
                onValueChange = { token = it; message = null },
                label = { Text("Bearer token") },
                supportingText = { Text("Paste the token without the Bearer prefix.") },
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                singleLine = true,
                enabled = !busy,
                modifier = Modifier.fillMaxWidth(),
            )
            Button(onClick = { update(false) }, shape = MaterialTheme.shapes.medium, enabled = !busy && store != null && endpointError == null) { Text("Save token") }
            TextButton(onClick = { removeTokenFor = server }, colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error), enabled = !busy && store != null && ConnectionInput.validServer(server)) { Text("Remove token") }
        }
        message?.let {
            StatusPanel(it, tone = StatusTone.NEUTRAL, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
        }
    }
}
