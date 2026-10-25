package com.trollm.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.trollm.app.StatusTone

/** Visual building blocks only: no navigation, storage or runtime ownership. */
@Composable
fun ScreenHeader(title: String, onBack: (() -> Unit)? = null, subtitle: String? = null) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (onBack != null) {
            TextButton(onClick = onBack) { Text("‹ Back") }
        }
        Text(title, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.semantics { heading() })
        subtitle?.takeIf(String::isNotBlank)?.let {
            Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
fun ContentCard(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Card(
        modifier = modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.large,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp), content = content)
    }
}

@Composable
fun SectionHeading(text: String, trailing: String? = null) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(text, style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f).semantics { heading() })
        trailing?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    }
}

@Composable
private fun statusColors(tone: StatusTone): Pair<Color, Color> {
    val colors = MaterialTheme.colorScheme
    return when (tone) {
        StatusTone.NEUTRAL -> colors.surfaceVariant to colors.onSurfaceVariant
        StatusTone.INFO -> colors.primaryContainer to colors.onPrimaryContainer
        StatusTone.SUCCESS -> colors.tertiaryContainer to colors.onTertiaryContainer
        StatusTone.WARNING -> colors.secondaryContainer to colors.onSecondaryContainer
        StatusTone.ERROR -> colors.errorContainer to colors.onErrorContainer
    }
}

@Composable
fun StatusBadge(text: String, tone: StatusTone = StatusTone.NEUTRAL, modifier: Modifier = Modifier) {
    val (background, foreground) = statusColors(tone)
    Surface(modifier = modifier, color = background, contentColor = foreground, shape = MaterialTheme.shapes.small) {
        Text(text, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp))
    }
}

@Composable
fun StatusPanel(title: String, message: String? = null, tone: StatusTone = StatusTone.NEUTRAL, modifier: Modifier = Modifier) {
    val (background, foreground) = statusColors(tone)
    Surface(modifier = modifier.fillMaxWidth(), color = background, contentColor = foreground, shape = MaterialTheme.shapes.large) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(title, style = MaterialTheme.typography.titleSmall)
            message?.takeIf(String::isNotBlank)?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
        }
    }
}
