package app.justyo.android

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.yo.ListKind
import uniffi.yo.ListRow

internal val BarHeight = 89.dp
private val BarFont = 36.sp
private val DeclineRed = Color(0xFFE74C3C)

internal class BarTapLock {
    private var order = emptyList<String>()
    private var positions = emptyMap<String, Int>()
    private val until = mutableMapOf<String, Long>()

    fun update(heights: Map<String, Int>): Boolean {
        val now = SystemClock.elapsedRealtime()
        until.values.removeAll { it <= now }
        val nextOrder = heights.keys.toList()
        var top = 0
        val nextPositions = heights.mapValues { (_, lines) -> top.also { top += lines } }
        val reordered = order != nextOrder
        nextOrder.forEachIndexed { index, key ->
            val previousTop = positions[key] ?: return@forEachIndexed
            val nextTop = nextPositions.getValue(key)
            if ((reordered && (order.getOrNull(index) != key || nextTop != previousTop)) || nextTop < previousTop) {
                until[key] = now + 500
            }
        }
        order = nextOrder
        positions = nextPositions
        return reordered
    }

    fun locked(key: String) = (until[key] ?: 0) > SystemClock.elapsedRealtime()
}

internal fun rowShade(friends: List<Friend>, index: Int): Color {
    val color = Math.floorMod(friends[index].color, RowColors.size)
    var start = index
    while (start > 0 && Math.floorMod(friends[start - 1].color, RowColors.size) == color) start--
    val shade = (index - start).let { 0.58f * (1f - 1f / (1f + it * .3f)) }
    return lerp(RowColors[color], Color.Black, shade)
}

internal data class BarContent(
    val friend: Friend,
    val row: ListRow?,
    val undo: UndoStack?,
    val media: List<Received>,
    val recording: Boolean,
    val thumbnails: List<OutgoingPreview>,
) {
    val received = friend.group == null && !friend.request && ((row?.yoTotal ?: 0u) > 0u || media.isNotEmpty())
    val sent = undo != null || row?.title == "Sent Yo" || thumbnails.isNotEmpty()
    val separate = received && (sent || recording || row?.title?.let { it != friend.name } == true)
    val senders = if (friend.group != null && !friend.request) row?.senders.orEmpty() else emptyList()
    val lines = if (friend.request) 2 else 1 + (if (separate) 1 else 0) + senders.size
}

@Composable internal fun FriendBar(
    repo: Repository, bar: BarContent, lines: Int, color: Color, camera: Boolean, left: Boolean,
    onPress: (Boolean) -> Unit, onTap: () -> Unit, onMenu: () -> Unit, onUndo: () -> Unit,
    onMedia: () -> Unit, onDecline: () -> Unit, onHold: ((Boolean) -> Unit)?,
) {
    val friend = bar.friend
    val resources = LocalResources.current
    val tapLabel = stringResource(if (friend.request) R.string.accept_and_yo else R.string.send_yo_action)
    val menuLabel = stringResource(R.string.contact_actions)
    val currentPress by rememberUpdatedState(onPress)
    val currentTap by rememberUpdatedState(onTap)
    val currentHold by rememberUpdatedState(onHold)
    val gesture = if (onHold != null) Modifier.pointerInput(friend.key) {
        awaitEachGesture {
            val down = awaitFirstDown()
            var timedOut = false
            val up = withTimeoutOrNull(300) { waitForUpOrCancellation() }
                ?: run { timedOut = currentEvent.changes.any { it.id == down.id && it.pressed && !it.isConsumed }; null }
            if (up != null) {
                up.consume()
                currentTap()
            } else if (timedOut) {
                currentHold?.invoke(true)
                try {
                    do {
                        val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id }
                        change?.consume()
                    } while (change?.pressed == true)
                } finally {
                    currentHold?.invoke(false)
                }
            }
        }
    }.semantics { onClick(label = tapLabel) { currentTap(); true } }
    else Modifier.combinedClickable(
        interactionSource = null, indication = null, role = Role.Button,
        onClick = onTap, onClickLabel = tapLabel, onLongClick = onMenu.takeUnless { friend.request },
        onLongClickLabel = menuLabel,
    )

    var pressed by remember(friend.key) { mutableStateOf(false) }
    val fill = remember(friend.key) { Animatable(0f) }
    LaunchedEffect(bar.recording) {
        fill.snapTo(0f)
        if (bar.recording) fill.animateTo(1f, tween(VideoLimitMs.toInt(), easing = LinearEasing))
    }
    var outgoingCount by remember(friend.key) { mutableLongStateOf(0L) }
    val heldMedia = bar.undo?.takeUnless { it.kind == ListKind.YO }
    SideEffect {
        if (heldMedia != null) outgoingCount = heldMedia.count.toLong()
        else if (!bar.sent) outgoingCount = 0L
    }
    val count = bar.undo?.count?.toLong()
        ?: maxOf(bar.row?.count?.toLong() ?: 1L, bar.thumbnails.size.toLong(), outgoingCount)
    val title = when {
        bar.recording -> stringResource(R.string.recording)
        bar.undo != null -> if (count <= 1) stringResource(R.string.sending_yo)
            else stringResource(R.string.sending_yos, count)
        bar.sent -> if (count <= 1) stringResource(R.string.sent_yo)
            else stringResource(R.string.sent_yos, count)
        else -> rowTitle(resources, bar.row, friend)
    }
    val extraReceive = bar.separate && lines >= 2
    val primaryReceived = bar.received && !extraReceive
    val currentThumb = bar.thumbnails.lastOrNull()
    var previousThumb by remember(friend.key) { mutableStateOf<OutgoingPreview?>(null) }
    SideEffect {
        if (currentThumb != null || !bar.sent) previousThumb = currentThumb
    }
    val thumb = (currentThumb ?: previousThumb).takeIf { bar.sent && !primaryReceived && bar.undo?.kind != ListKind.YO }
    val showUndo = bar.undo != null && !primaryReceived
    val hasSign = bar.media.isNotEmpty() && !friend.request
    val showSignOnPrimary = hasSign && !extraReceive
    val outgoingInset = if (showUndo || thumb != null) 88 else 20
    val primarySignInset = if (showSignOnPrimary) 68 else 20
    val receivedSignInset = if (hasSign) 68 else 20
    val start = if (left) Alignment.CenterEnd else Alignment.CenterStart
    val end = if (left) Alignment.CenterStart else Alignment.CenterEnd
    val receivedCount = if (hasSign) maxOf(bar.row?.markCount?.toLong() ?: 0L, bar.media.size.toLong())
        else bar.row?.yoTotal?.toLong() ?: 1L
    val received = yoWords(receivedCount)
    val receivedFull = stringResource(R.string.yo_from, received, friend.name)
    val fadeElapsed = bar.row?.fadeElapsed
    val fadeDuration = remember { uniffi.yo.listTimings().yoFadeMs.toLong() }
    val receivedOpacity = remember(friend.key) { Animatable(1f) }
    LaunchedEffect(bar.row?.yoToken, fadeElapsed, bar.row?.clockRunning) {
        val elapsed = fadeElapsed?.toLong() ?: 0L
        receivedOpacity.snapTo(1f - (elapsed.toFloat() / fadeDuration).coerceIn(0f, 1f))
        if (fadeElapsed != null && bar.row.clockRunning) {
            receivedOpacity.animateTo(0f, tween((fadeDuration - elapsed).coerceAtLeast(0).toInt(), easing = LinearEasing))
        }
    }
    val mediaSign: @Composable () -> Unit = {
        Box(Modifier.padding(horizontal = 10.dp).width(60.dp).height(BarHeight).clickable(
            interactionSource = null, indication = null, role = Role.Button, onClick = onMedia,
        ).semantics { contentDescription = resources.getQuantityString(R.plurals.media_items, bar.media.size, bar.media.size) },
            contentAlignment = Alignment.Center) {
            val sign = bar.row?.mediaSign.orEmpty().ifEmpty { if (bar.media.first().video) "🎥" else "📷" }
            Text(sign, fontSize = BarFont)
        }
    }
    Column(Modifier.fillMaxWidth().pointerInput(friend.key) {
        awaitEachGesture {
            awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            pressed = true
            currentPress(true)
            try {
                do {
                    val event = awaitPointerEvent(PointerEventPass.Final)
                } while (event.changes.any { it.pressed })
            } finally {
                pressed = false
                currentPress(false)
            }
        }
    }.drawBehind {
        drawRect(color.copy(alpha = if (camera) .45f else 1f))
        if (bar.recording) {
            val width = size.width * fill.value
            val x = if (layoutDirection == LayoutDirection.Ltr) 0f else size.width - width
            drawRect(DeclineRed.copy(alpha = .8f), Offset(x, 0f), Size(width, BarHeight.toPx()))
        }
        if (pressed) drawRect(Color.White.copy(alpha = .12f))
    }) {
        Box(Modifier.fillMaxWidth().height(BarHeight).then(gesture), contentAlignment = Alignment.Center) {
            if (primaryReceived && !hasSign && fadeElapsed != null) {
                BarWords(title, modifier = Modifier.graphicsLayer { alpha = 1f - receivedOpacity.value })
            }
            BarWords(
                if (primaryReceived) receivedFull else title,
                fallback = received.takeIf { primaryReceived },
                startInset = if (left) primarySignInset else outgoingInset,
                endInset = if (left) outgoingInset else primarySignInset,
                modifier = if (primaryReceived && !hasSign) Modifier.graphicsLayer { alpha = receivedOpacity.value } else Modifier,
            )
            if (showUndo || thumb != null) Box(Modifier.align(start).padding(horizontal = 20.dp)) {
                OutgoingThumbnail(repo, thumb, bar.undo, onUndo)
            }
            if (showSignOnPrimary) Box(Modifier.align(end)) { mediaSign() }
        }
        if (friend.request) {
            Row(Modifier.fillMaxWidth().height(BarHeight)) {
                if (left) RequestChoice("Yo", color, Modifier.weight(2f), onTap)
                RequestChoice(stringResource(R.string.ui_no), DeclineRed, Modifier.weight(1f), onDecline)
                if (!left) RequestChoice("Yo", color, Modifier.weight(2f), onTap)
            }
        } else {
            if (extraReceive) Box(Modifier.fillMaxWidth().height(BarHeight).then(gesture), contentAlignment = Alignment.Center) {
                BarWords(
                    receivedFull,
                    fallback = received,
                    modifier = if (!hasSign) Modifier.graphicsLayer { alpha = receivedOpacity.value } else Modifier,
                    startInset = if (left) receivedSignInset else outgoingInset,
                    endInset = if (left) outgoingInset else receivedSignInset,
                )
                if (hasSign) Box(Modifier.align(end)) { mediaSign() }
            }
            repeat(lines - 1 - if (extraReceive) 1 else 0) { index ->
                Box(Modifier.fillMaxWidth().height(BarHeight).then(gesture), contentAlignment = Alignment.Center) {
                    bar.senders.getOrNull(index)?.let { sender ->
                        val solid = remember { uniffi.yo.listTimings().yoSolidMs.toLong() }
                        val opacity = remember(sender.user) { Animatable(1f) }
                        LaunchedEffect(sender.elapsed, bar.row?.clockRunning) {
                            val elapsed = (sender.elapsed.toLong() - solid).coerceAtLeast(0)
                            opacity.snapTo(1f - (elapsed.toFloat() / fadeDuration).coerceIn(0f, 1f))
                            if (sender.elapsed.toLong() >= solid && bar.row?.clockRunning == true) {
                                opacity.animateTo(0f, tween((fadeDuration - elapsed).coerceAtLeast(0).toInt(), easing = LinearEasing))
                            }
                        }
                        BarWords(stringResource(R.string.group_sender_yo, sender.name, yoWords(sender.count.toLong())),
                            modifier = Modifier.graphicsLayer { alpha = opacity.value })
                    }
                }
            }
        }
    }
}

@Composable private fun RequestChoice(text: String, color: Color, modifier: Modifier, onTap: () -> Unit) {
    Box(modifier.fillMaxHeight().background(color).clickable(
        interactionSource = null, indication = null, role = Role.Button, onClick = onTap,
    ), contentAlignment = Alignment.Center) { BarWords(text) }
}

@Composable private fun yoWords(count: Long) =
    if (count <= 1) "Yo" else stringResource(R.string.counted_yo, count)

private val CountDigits = Regex("[0-9]+(?=×)")

@Composable
private fun BarWords(
    text: String,
    modifier: Modifier = Modifier,
    fallback: String? = null,
    startInset: Int = 20,
    endInset: Int = 20,
) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val style = TextStyle(fontFamily = YoFont, fontSize = BarFont, color = Color.White)
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val start = startInset.dp
        val end = endInset.dp
        val available = (maxWidth - start - end).coerceAtLeast(0.dp)
        fun width(value: String) = measurer.measure(value, style, maxLines = 1, softWrap = false).size.width
        val slot = ('0'..'9').maxOf { width(it.toString()) }
        fun parts(value: String): Triple<String, String, String> {
            val count = CountDigits.find(value) ?: return Triple(value, "", "")
            return Triple(value.substring(0, count.range.first), count.value, value.substring(count.range.last + 1))
        }
        fun digitBlock(digits: String) = if (digits.isEmpty()) 0 else slot * maxOf(2, digits.length)
        fun total(value: String) = with(density) {
            val (prefix, digits, suffix) = parts(value)
            (width(prefix) + digitBlock(digits) + width(suffix)).toDp()
        }
        val selected = if (fallback != null && total(text) > available) fallback else text
        val (prefix, digits, suffix) = parts(selected)
        val lineWidth = total(selected).coerceAtMost(available)
        val lineStart = ((maxWidth - lineWidth) / 2).coerceIn(start, (maxWidth - end - lineWidth).coerceAtLeast(start))
        Row(
            Modifier.offset(x = lineStart).width(lineWidth).clipToBounds()
                .clearAndSetSemantics { this.contentDescription = selected },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (prefix.isNotEmpty()) Text(prefix, style = style, maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis)
            if (digits.isNotEmpty()) {
                Box(Modifier.width(with(density) { digitBlock(digits).toDp() }), contentAlignment = Alignment.CenterEnd) {
                    Text(digits, style = style, maxLines = 1, softWrap = false)
                }
                Text(suffix, style = style, maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}
