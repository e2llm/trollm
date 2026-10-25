package com.trollm.app.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Surface
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private val LightColors = lightColorScheme(
    primary = Color(0xFF5B45B0), onPrimary = Color.White,
    primaryContainer = Color(0xFFEEEBF9), onPrimaryContainer = Color(0xFF493487),
    secondary = Color(0xFF70500B), onSecondary = Color.White,
    secondaryContainer = Color(0xFFFFF2D7), onSecondaryContainer = Color(0xFF70500B),
    tertiary = Color(0xFF245B3B), onTertiary = Color.White,
    tertiaryContainer = Color(0xFFE8F4EC), onTertiaryContainer = Color(0xFF245B3B),
    background = Color(0xFFF6F7FA), onBackground = Color(0xFF202431),
    surface = Color.White, onSurface = Color(0xFF202431),
    surfaceVariant = Color(0xFFEBEDF2), onSurfaceVariant = Color(0xFF626978),
    outline = Color(0xFF777E8D), outlineVariant = Color(0xFFE0E3EB),
    error = Color(0xFFA32C2A), onError = Color.White,
    errorContainer = Color(0xFFFDEDEC), onErrorContainer = Color(0xFF85201F),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFFCBBDFF), onPrimary = Color(0xFF302251),
    primaryContainer = Color(0xFF342D49), onPrimaryContainer = Color(0xFFDDD1FF),
    secondary = Color(0xFFF5D58F), onSecondary = Color(0xFF392B0B),
    secondaryContainer = Color(0xFF423820), onSecondaryContainer = Color(0xFFF5D58F),
    tertiary = Color(0xFFA6DFB8), onTertiary = Color(0xFF143522),
    tertiaryContainer = Color(0xFF253E2E), onTertiaryContainer = Color(0xFFA6DFB8),
    background = Color(0xFF16181F), onBackground = Color(0xFFF0F0F7),
    surface = Color(0xFF22252F), onSurface = Color(0xFFF0F0F7),
    surfaceVariant = Color(0xFF303440), onSurfaceVariant = Color(0xFFBCC2D0),
    outline = Color(0xFF8D95A5), outlineVariant = Color(0xFF3D4351),
    error = Color(0xFFFFB4B0), onError = Color(0xFF5C1314),
    errorContainer = Color(0xFF4F282A), onErrorContainer = Color(0xFFFFDAD7),
)

private val TrollmTypography = Typography(
    headlineMedium = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 28.sp, lineHeight = 36.sp),
    headlineSmall = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 24.sp, lineHeight = 32.sp),
    titleLarge = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 20.sp, lineHeight = 28.sp),
    titleMedium = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 16.sp, lineHeight = 24.sp),
    titleSmall = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, lineHeight = 20.sp),
    bodyLarge = TextStyle(fontFamily = FontFamily.Default, fontSize = 16.sp, lineHeight = 24.sp),
    bodyMedium = TextStyle(fontFamily = FontFamily.Default, fontSize = 14.sp, lineHeight = 21.sp),
    bodySmall = TextStyle(fontFamily = FontFamily.Default, fontSize = 12.sp, lineHeight = 18.sp),
    labelLarge = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 14.sp, lineHeight = 20.sp),
    labelMedium = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 12.sp, lineHeight = 16.sp),
    labelSmall = TextStyle(fontFamily = FontFamily.Default, fontWeight = FontWeight.SemiBold, fontSize = 11.sp, lineHeight = 16.sp),
)

@Composable
fun TrollmTheme(darkTheme: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (darkTheme) DarkColors else LightColors,
        typography = TrollmTypography,
        shapes = Shapes(
            extraSmall = RoundedCornerShape(4.dp), small = RoundedCornerShape(8.dp),
            medium = RoundedCornerShape(12.dp), large = RoundedCornerShape(16.dp),
            extraLarge = RoundedCornerShape(20.dp),
        ),
    ) {
        Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            // Apply once around the existing screens; inset padding consumes the
            // covered area so the keyboard and system bars do not double-pad it.
            Box(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) { content() }
        }
    }
}
