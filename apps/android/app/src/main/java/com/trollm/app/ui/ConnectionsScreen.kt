package com.trollm.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
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
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.trollm.app.ConnectionInput
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
    var busy by remember { mutableStateOf(false) }
    var store by remember { mutableStateOf<SecretsStore?>(null) }
    var oauth by remember { mutableStateOf<OAuthConnections?>(null) }
    var oauthStatus by remember { mutableStateOf("Opening connection storage…") }
    var refreshStatus by remember { mutableStateOf(0) }

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
            } catch (_: Exception) {
                message = "Could not read connection status."
                oauthStatus = "Check the HTTPS MCP endpoint and connection storage."
            }
        }
    }

    fun update(remove: Boolean) {
        if (busy) return
        val secrets = store ?: return
        if (!ConnectionInput.validServer(server)) {
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
            message = "Enter a valid HTTPS MCP endpoint before saving a token."
            return
        }
        val name = server
        val value = if (remove) null else token
        token = ""
        busy = true
        message = null
        scope.launch {
            try {
                withContext(Dispatchers.IO) { secrets.setToken(name, url, value) }
                saved = !remove
                savedForEndpoint = !remove
                message = if (remove) "Token removed." else "Token saved."
            } catch (error: kotlinx.coroutines.CancellationException) {
                throw error
            } catch (_: Exception) {
                saved = null
                message = "Could not update connection."
            } finally {
                busy = false
            }
        }
    }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp)) {
        Text("Connections", style = MaterialTheme.typography.headlineSmall)
        Text("Use the connection name from your flow.")
        OutlinedTextField(
            value = server,
            onValueChange = { server = it; token = ""; message = null },
            label = { Text("Connection name") },
            singleLine = true,
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = endpoint,
            onValueChange = { endpoint = it; message = null },
            label = { Text("MCP HTTPS endpoint") },
            singleLine = true,
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(oauthStatus)
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
                } catch (_: Exception) {
                    message = "Could not start sign-in. Check the endpoint, trust configuration and browser."
                } finally { busy = false }
            }
        }, enabled = !busy && oauth != null) { Text("Sign in with browser") }
        Button(onClick = {
            val client = oauth ?: return@Button
            val name = server
            busy = true
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { client.logout(name) }
                    message = "OAuth account signed out. Manual tokens are managed separately."
                } catch (error: kotlinx.coroutines.CancellationException) {
                    throw error
                } catch (_: Exception) { message = "Could not sign out of this connection." }
                finally { busy = false }
            }
        }, enabled = !busy && oauth != null && ConnectionInput.validServer(server)) { Text("Sign out of OAuth") }
        Text("Manual Bearer token (saving switches this connection away from OAuth).")
        Text(when (saved) {
            true -> if (savedForEndpoint) "Saved token available for this endpoint."
                else "Token stored but inactive for this endpoint. Re-save it with its intended endpoint to use manual authentication."
            false -> "No saved token."
            null -> "Enter a connection name to check saved-token status."
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
        Button(onClick = { update(false) }, enabled = !busy && store != null) { Text("Save token") }
        Button(onClick = { update(true) }, enabled = !busy && store != null) { Text("Remove token") }
        message?.let { Text(it) }
        Button(onClick = { token = ""; onBack() }) { Text("Back") }
    }
}
