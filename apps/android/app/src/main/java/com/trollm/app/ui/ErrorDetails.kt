package com.trollm.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.key
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.text.font.FontFamily
import com.trollm.app.UiError

@Composable
fun TechnicalDetails(details: String) {
    var expanded by remember(details) { mutableStateOf(false) }
    Button(onClick = { expanded = !expanded }) { Text(if (expanded) "Hide details" else "Details") }
    if (expanded) key(details) {
        Text(details, modifier = Modifier.heightIn(max = 200.dp).verticalScroll(rememberScrollState()),
            fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
    }
}

@Composable
fun OperationError(error: UiError) {
    Column {
        Text(error.summary, color = MaterialTheme.colorScheme.error)
        error.details?.let { TechnicalDetails(it) }
    }
}
