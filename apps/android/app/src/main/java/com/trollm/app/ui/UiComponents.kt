package com.trollm.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import com.trollm.app.StatusTone

/** Visual building blocks only: no navigation, storage or runtime ownership. */
@Composable
fun ScreenHeader(
    title: String,
    onBack: (() -> Unit)? = null,
    subtitle: String? = null,
    actions: (@Composable () -> Unit)? = null,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 48.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            onBack?.let { UiIconButton("Navigate up", UiIcon.Up, it) }
            Text(
                title,
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.weight(1f).semantics { heading() },
            )
            actions?.invoke()
        }
        subtitle?.takeIf(String::isNotBlank)?.let {
            Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
fun OverflowMenu(
    description: String,
    content: @Composable ColumnScope.(dismiss: () -> Unit) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    val dismiss = { expanded = false }
    Box {
        UiIconButton(
            description,
            UiIcon.More,
            onClick = { expanded = true },
            modifier = Modifier.semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" },
        )
        DropdownMenu(expanded = expanded, onDismissRequest = dismiss) { content(dismiss) }
    }
}

@Composable
fun DisclosureRow(label: String, expanded: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
            .padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(label, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
        UiGlyph(if (expanded) UiIcon.Collapse else UiIcon.Expand)
    }
}

@Composable
internal fun DetailsButton(description: String, onClick: () -> Unit) {
    UiIconButton(description, UiIcon.Details, onClick)
}

private enum class UiIcon { Up, More, Expand, Collapse, Details }

@Composable
private fun UiIconButton(
    description: String,
    icon: UiIcon,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    IconButton(
        onClick = onClick,
        modifier = modifier.size(48.dp).semantics { contentDescription = description },
    ) { UiGlyph(icon) }
}

@Composable
private fun UiGlyph(icon: UiIcon) {
    val color = LocalContentColor.current
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    Canvas(Modifier.size(24.dp)) {
        val stroke = 2.dp.toPx()
        fun point(x: Float, y: Float) = Offset(size.width * x / 24f, size.height * y / 24f)
        fun line(x1: Float, y1: Float, x2: Float, y2: Float) =
            drawLine(color, point(x1, y1), point(x2, y2), stroke, StrokeCap.Round)

        when (icon) {
            UiIcon.Up -> {
                val tip = if (rtl) 19f else 5f
                val tail = if (rtl) 5f else 19f
                line(tip, 12f, tail, 12f)
                line(tip, 12f, 12f, 5f)
                line(tip, 12f, 12f, 19f)
            }
            UiIcon.More -> for (y in listOf(5f, 12f, 19f)) drawCircle(color, stroke * 0.9f, point(12f, y))
            UiIcon.Expand, UiIcon.Collapse -> {
                val edge = if (icon == UiIcon.Expand) 9f else 15f
                val middle = if (icon == UiIcon.Expand) 15f else 9f
                line(6f, edge, 12f, middle)
                line(12f, middle, 18f, edge)
            }
            UiIcon.Details -> {
                drawCircle(color, 8.dp.toPx(), point(12f, 12f), style = Stroke(stroke))
                drawCircle(color, stroke * 0.6f, point(12f, 8f))
                line(12f, 12f, 12f, 16f)
            }
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
        Text(text, style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f).semantics { heading() })
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
        Row(
            Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            StatusGlyph(tone)
            Text(text, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f, fill = false))
        }
    }
}

@Composable
fun StatusPanel(
    title: String,
    message: String? = null,
    tone: StatusTone = StatusTone.NEUTRAL,
    modifier: Modifier = Modifier,
    prominent: Boolean = false,
) {
    val (background, foreground) = statusColors(tone)
    Surface(modifier = modifier.fillMaxWidth(), color = background, contentColor = foreground, shape = MaterialTheme.shapes.large) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                StatusGlyph(tone)
                Text(title, style = if (prominent) MaterialTheme.typography.headlineSmall
                    else MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            }
            message?.takeIf(String::isNotBlank)?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
        }
    }
}

@Composable
private fun StatusGlyph(tone: StatusTone) {
    val color = LocalContentColor.current
    Canvas(Modifier.size(16.dp)) {
        val stroke = 1.5.dp.toPx()
        fun point(x: Float, y: Float) = Offset(size.width * x / 16f, size.height * y / 16f)
        fun line(x1: Float, y1: Float, x2: Float, y2: Float) =
            drawLine(color, point(x1, y1), point(x2, y2), stroke, StrokeCap.Round)

        drawCircle(color, 6.dp.toPx(), style = Stroke(stroke))
        when (tone) {
            StatusTone.NEUTRAL -> line(5.5f, 8f, 10.5f, 8f)
            StatusTone.INFO -> drawCircle(color, stroke, center)
            StatusTone.SUCCESS -> {
                line(5f, 8f, 7f, 10f)
                line(7f, 10f, 11f, 6f)
            }
            StatusTone.WARNING, StatusTone.ERROR -> {
                line(8f, 5f, 8f, 8.5f)
                drawCircle(color, stroke / 2f, point(8f, 11f))
            }
        }
    }
}
