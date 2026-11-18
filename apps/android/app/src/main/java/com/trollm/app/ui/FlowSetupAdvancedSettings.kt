package com.trollm.app.ui

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.trollm.app.FlowSetup
import com.trollm.app.FlowSetupOverrides
import com.trollm.app.ModelOverrideDraft
import com.trollm.app.modelDisplayLabel

@Composable
fun FlowSetupAdvancedSettings(
    defaults: FlowSetup,
    effective: FlowSetup,
    resetKey: Int,
    enabled: Boolean,
    onSave: (FlowSetup) -> Unit,
    onRestore: () -> Unit,
) {
    var models by remember(effective, resetKey) { mutableStateOf(effective.models.mapValues { (_, model) -> ModelOverrideDraft.from(model) }) }
    var connections by remember(effective, resetKey) { mutableStateOf(effective.connections.mapValues { (_, connection) -> connection.url }) }
    var error by remember(effective, resetKey) { mutableStateOf<String?>(null) }
    ContentCard {
        SectionHeading("Advanced settings")
        Text(if (effective.models.isEmpty()) "Connection endpoints. Changes are validated before saving." else "Model identities, context and sampling settings. Changes are validated before saving.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (!enabled) Text(if (effective.models.isEmpty()) "Settings are being checked." else "Settings are locked while models are downloading or being checked.")
        effective.models.forEach { (slot, _) ->
            Text(effective.modelDisplayLabel(slot, defaults), style = MaterialTheme.typography.titleMedium)
            val draft = models.getValue(slot)
            fun update(value: ModelOverrideDraft) { models = models + (slot to value); error = null }
            SettingField("Model URL", draft.url, enabled) { update(draft.copy(url = it)) }
            SettingField("SHA-256 checksum", draft.sha256, enabled) { update(draft.copy(sha256 = it)) }
            SettingField("Expected size (bytes)", draft.sizeBytes, enabled) { update(draft.copy(sizeBytes = it)) }
            SettingField("Context window (tokens)", draft.ctx, enabled) { update(draft.copy(ctx = it)) }
            SettingField("Temperature", draft.temperature, enabled) { update(draft.copy(temperature = it)) }
            SettingField("Top P", draft.topP, enabled) { update(draft.copy(topP = it)) }
        }
        effective.connections.forEach { (name, _) ->
            OutlinedTextField(value = connections.getValue(name), onValueChange = { connections = connections + (name to it); error = null },
                enabled = enabled, label = { Text("$name endpoint URL") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        Button(shape = MaterialTheme.shapes.medium, enabled = enabled, onClick = {
            try {
                val updated = defaults.copy(
                    models = defaults.models.mapValues { (slot, model) -> models.getValue(slot).applyTo(model) },
                    connections = defaults.connections.mapValues { (name, connection) -> connection.copy(url = connections.getValue(name)) },
                )
                // Validate connection URLs and preserve pinned descriptor metadata.
                onSave(FlowSetupOverrides.between(defaults, updated).applyTo(defaults))
                error = null
            } catch (failure: IllegalArgumentException) {
                error = failure.message ?: "Check the advanced settings."
            } catch (_: Exception) {
                error = "Check the advanced settings."
            }
        }) { Text("Save settings") }
        OutlinedButton(shape = MaterialTheme.shapes.medium, enabled = enabled, onClick = { error = null; onRestore() }) { Text("Restore defaults") }
    }
}

@Composable
private fun SettingField(label: String, value: String, enabled: Boolean, onChange: (String) -> Unit) {
    OutlinedTextField(value = value, onValueChange = onChange, enabled = enabled,
        label = { Text(label) }, singleLine = true, modifier = Modifier.fillMaxWidth())
}
