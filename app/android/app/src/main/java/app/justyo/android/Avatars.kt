package app.justyo.android

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.util.LruCache
import androidx.compose.ui.graphics.toArgb
import androidx.core.content.res.ResourcesCompat
import androidx.core.graphics.createBitmap
import androidx.core.graphics.drawable.IconCompat
import kotlin.math.roundToInt

/** Share the same adaptive icon between the launcher and the people in a notification. */
internal object Avatars {
    private val bitmaps = LruCache<Pair<String, Int>, Bitmap>(24)

    fun icon(context: Context, name: String, color: Int?): IconCompat {
        val label = name.removePrefix("#").trim()
        val initial = if (label.isEmpty()) "Y" else String(Character.toChars(label.codePointAt(0))).uppercase()
        val background = (color?.let { RowColors[Math.floorMod(it, RowColors.size)] } ?: Amethyst).toArgb()
        val key = initial to background
        val density = context.resources.displayMetrics.density
        val size = (108 * density).roundToInt()
        val bitmap = bitmaps.get(key)?.takeIf { it.width == size } ?: createBitmap(size, size).apply {
            val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                this.color = Color.WHITE
                typeface = ResourcesCompat.getFont(context, R.font.montserrat_bold)
                textAlign = Paint.Align.CENTER
                textSize = 36 * density
            }
            Canvas(this).apply {
                drawColor(background)
                drawText(initial, size / 2f, (size - paint.ascent() - paint.descent()) / 2f, paint)
            }
            bitmaps.put(key, this)
        }
        return IconCompat.createWithAdaptiveBitmap(bitmap)
    }
}
