package app.justyo.android

import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight

val Amethyst=Color(0xFF9B59B6)
val YoFont=FontFamily(Font(R.font.montserrat_bold,FontWeight.Bold))
val RowColors=listOf(0xFF1ABC9C,0xFF2ECC71,0xFF3498DB,0xFF34495E,0xFF16A085,0xFFF1C40F,0xFF2980B9,0xFF8E44AD).map { Color(it) }
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable fun YoTheme(content: @Composable () -> Unit) {
    MaterialTheme(motionScheme=MotionScheme.expressive(),colorScheme=darkColorScheme(primary=Color.White,onPrimary=Amethyst,secondary=Amethyst,
        background=Amethyst,onBackground=Color.White,surface=Color(0xFF27232B),onSurface=Color.White),content=content)
}
