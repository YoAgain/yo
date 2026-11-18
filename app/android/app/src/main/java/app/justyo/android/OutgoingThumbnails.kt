package app.justyo.android

import android.graphics.BitmapFactory
import android.os.SystemClock
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Undo
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.Icon
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

@Composable internal fun OutgoingThumbnail(repo: Repository, previews: List<OutgoingPreview>, undo: UndoStack?, onUndo: () -> Unit) {
    val description = stringResource(if (undo != null) R.string.ui_undo_send else R.string.outgoing_photo)
    Box(Modifier.size(48.dp).then(
        if (undo != null) Modifier.clickable(interactionSource = null, indication = null, role = Role.Button,
            onClickLabel = description, onClick = onUndo) else Modifier
    ), contentAlignment = Alignment.Center) {
        val pictures = previews.takeLast(3)
        pictures.forEachIndexed { index, preview -> key(preview.id) {
            val image by produceState<android.graphics.Bitmap?>(null, preview.id) {
                value = withContext(Dispatchers.IO) {
                    repo.media.thumbnail(preview.id)?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }
                }
            }
            val offset = (index * 4 - (pictures.size - 1) * 2).dp
            image?.let {
                Image(it.asImageBitmap(), description, Modifier.offset(x = offset, y = -offset)
                    .size(if (pictures.size > 1) 36.dp else 44.dp), contentScale = ContentScale.Crop)
            }
        } }
        if (pictures.lastOrNull()?.video == true || undo?.video == true) Icon(Icons.Default.PlayArrow, description, tint = Color.White)
        else if (pictures.isEmpty() && undo != null) Icon(Icons.AutoMirrored.Filled.Undo, description, tint = Color.White)
        if (undo != null) {
            val fraction = remember(undo) { mutableFloatStateOf(undo.fraction(SystemClock.elapsedRealtime())) }
            LaunchedEffect(undo) {
                if (undo.pausedRemaining == null) {
                    while (fraction.floatValue > 0f) {
                        withFrameNanos { fraction.floatValue = undo.fraction(SystemClock.elapsedRealtime()) }
                    }
                }
            }
            Canvas(Modifier.align(Alignment.BottomCenter).width(44.dp).height(2.dp)) {
                drawRect(Color.White, size = Size(size.width * fraction.floatValue, size.height))
            }
        }
    }
}
