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
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.yo.ListFlowWidths
import uniffi.yo.ListKind
import uniffi.yo.ListLine
import uniffi.yo.ListRow
import uniffi.yo.ListSender
import uniffi.yo.listFlowTwoLines
import uniffi.yo.listSenderFlow
import uniffi.yo.productArrivalLines
import uniffi.yo.productCounted
import uniffi.yo.productMediaSign
import uniffi.yo.productSendWords

internal val BarHeight = 89.dp
private val BarFont = 36.sp
/** A group's senders, two lines in one bar: the second, fixed size (iOS's too: 24 of 36 in an 89 bar). */
private val BarSmallFont = 24.sp
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

/**
 * Everything a bar shows. Immutable (read-only lists, never changed after it is made): Compose compares it by value, so
 * a bar whose content is the same skips its recomposition when the list recomposes (a press, a viewport change).
 */
@Immutable
internal data class BarContent(
    val friend: Friend,
    val row: ListRow?,
    val undo: UndoStack?,
    val media: List<Received>,
    val recording: Boolean,
    val thumbnails: List<OutgoingPreview>,
) {
    /** The core's one line: the title, or the Yos that came after the last send (never both, a bar never grows). */
    val line: ListLine = if (friend.request) ListLine.Title else row?.line ?: ListLine.Title
    val received = line !is ListLine.Title && !recording
    val sent = undo != null || row?.title == copy(uniffi.yo.ProductText.SENT_YO) || thumbnails.isNotEmpty()
    val lines = if (friend.request) 2 else 1
}

@Composable internal fun FriendBar(
    repo: Repository, bar: BarContent, color: Color, camera: Boolean, left: Boolean,
    onPress: (Boolean) -> Unit, onTap: () -> Unit, onMenu: () -> Unit, onUndo: () -> Unit,
    onMedia: () -> Unit, onDecline: () -> Unit, onHold: ((Boolean) -> Unit)?,
    onMediaTouch: (MediaTouch) -> Unit = {},
) {
    val friend = bar.friend
    val resources = LocalResources.current
    val tapLabel = stringResource(if (friend.request) R.string.accept_and_yo else R.string.send_yo_action)
    val menuLabel = stringResource(R.string.contact_actions)
    val currentPress by rememberUpdatedState(onPress)
    val currentTap by rememberUpdatedState(onTap)
    val currentHold by rememberUpdatedState(onHold)
    val currentMedia by rememberUpdatedState(onMedia)
    val currentMediaTouch by rememberUpdatedState(onMediaTouch)
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
        else if (bar.thumbnails.isNotEmpty()) outgoingCount = maxOf(outgoingCount, bar.thumbnails.size.toLong())
        else if (!bar.sent) outgoingCount = 0L
    }
    val count = bar.undo?.count?.toLong()
        ?: maxOf(bar.row?.count?.toLong() ?: 1L, bar.thumbnails.size.toLong(), outgoingCount)
    val title = when {
        bar.recording -> stringResource(R.string.recording)
        bar.undo != null || bar.thumbnails.isNotEmpty() -> productSendWords(productCounted(count), true, Words)
        bar.sent -> productSendWords(productCounted(count), false, Words)
        else -> rowTitle(bar.row, friend)
    }
    val primaryReceived = bar.received
    var previousThumbnails by remember(friend.key) { mutableStateOf(emptyList<OutgoingPreview>()) }
    SideEffect {
        if (!bar.sent || bar.undo != null || bar.thumbnails.size >= previousThumbnails.size) {
            previousThumbnails = bar.thumbnails
        }
    }
    val thumbnails = (if (bar.thumbnails.size >= previousThumbnails.size) bar.thumbnails else previousThumbnails)
        .takeIf { bar.sent && !primaryReceived && bar.undo?.kind != ListKind.YO }.orEmpty()
    val showUndo = bar.undo != null && !primaryReceived
    val hasSign = bar.media.isNotEmpty() && !friend.request
    val showSignOnPrimary = hasSign
    val outgoingInset = if (showUndo || thumbnails.isNotEmpty()) 88 else 20
    val primarySignInset = if (showSignOnPrimary) 68 else 20
    val start = if (left) Alignment.CenterEnd else Alignment.CenterStart
    val end = if (left) Alignment.CenterStart else Alignment.CenterEnd
    val arrival = bar.line as? ListLine.Arrival
    val receivedCount = maxOf(arrival?.count?.toLong() ?: 1L, if (hasSign) bar.media.size.toLong() else 0L)
    val from = arrival?.name.orEmpty()
    val receivedFull = remember(receivedCount, from) { productArrivalLines(receivedCount, from, Words) }
    val senders = (bar.line as? ListLine.Senders)?.senders
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
    // A finger on the sign shows the photos at once: a short tap lets them run, let go after holding closes them.
    // Held and drawn down out of the bar, the picture zooms around the finger (MediaViewer).
    val mediaSign: @Composable () -> Unit = {
        var signBox by remember { mutableStateOf<LayoutCoordinates?>(null) }
        Box(Modifier.padding(end = if (left) 0.dp else 20.dp, start = if (left) 20.dp else 0.dp).width(40.dp).height(BarHeight)
            .onGloballyPositioned { signBox = it }
            .pointerInput(friend.key) {
                awaitEachGesture {
                    val down = awaitFirstDown()
                    down.consume()
                    currentMedia()
                    var ended = false
                    try {
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                            change.consume()  // the list does not scroll under a held finger
                            if (!change.pressed) {
                                ended = true
                                currentMediaTouch(
                                    if (change.uptimeMillis - down.uptimeMillis < 350) MediaTouch.Tapped else MediaTouch.Released)
                                break
                            }
                            signBox?.takeIf { it.isAttached }?.let { box ->
                                currentMediaTouch(MediaTouch.Moved(box.localToScreen(change.position),
                                    box.localToScreen(Offset(0f, box.size.height.toFloat())).y))
                            }
                        }
                    } finally {
                        if (!ended) currentMediaTouch(MediaTouch.Tapped)  // taken away: they run on, unzoomed
                    }
                }
            }
            .semantics {
                role = Role.Button
                onClick { currentMedia(); true }
                contentDescription = resources.getQuantityString(R.plurals.media_items, bar.media.size, bar.media.size)
            },
            contentAlignment = Alignment.Center) {
            val sign = productMediaSign(if (bar.media.any { it.video }) ListKind.VIDEO else ListKind.PHOTO)
            Text(sign, style = TextStyle(fontSize = 26.sp), maxLines = 1)
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
            val receivedModifier =
                if (primaryReceived && !hasSign) Modifier.graphicsLayer { alpha = receivedOpacity.value } else Modifier
            if (primaryReceived && senders != null) SenderFlow(
                senders,
                startInset = if (left) primarySignInset else outgoingInset,
                endInset = if (left) outgoingInset else primarySignInset,
                modifier = receivedModifier,
            ) else BarWords(
                if (primaryReceived) receivedFull else listOf(title),
                startInset = if (left) primarySignInset else outgoingInset,
                endInset = if (left) outgoingInset else primarySignInset,
                modifier = receivedModifier,
            )
            if (showUndo || thumbnails.isNotEmpty()) Box(Modifier.align(start).padding(horizontal = 20.dp)) {
                OutgoingThumbnail(repo, thumbnails, bar.undo, onUndo)
            }
            if (showSignOnPrimary) Box(Modifier.align(end)) { mediaSign() }
        }
        if (friend.request) {
            Row(Modifier.fillMaxWidth().height(BarHeight)) {
                if (left) RequestChoice("Yo", color, Modifier.weight(2f), onTap)
                RequestChoice(stringResource(R.string.ui_no), DeclineRed, Modifier.weight(1f), onDecline)
                if (!left) RequestChoice("Yo", color, Modifier.weight(2f), onTap)
            }
        }
    }
}

@Composable private fun RequestChoice(text: String, color: Color, modifier: Modifier, onTap: () -> Unit) {
    Box(modifier.fillMaxHeight().background(color).clickable(
        interactionSource = null, indication = null, role = Role.Button, onClick = onTap,
    ), contentAlignment = Alignment.Center) { BarWords(text) }
}

private val CountDigits = Regex("[0-9]+(?=×)")

@Composable
private fun BarWords(text: String, modifier: Modifier = Modifier, startInset: Int = 20, endInset: Int = 20) =
    BarWords(listOf(text), modifier, startInset, endInset)

/**
 * A group's senders since its bar last showed its name, newest first, over two lines at the small size ("Yo from TOM,
 * IDA, BEN (3×)," / "SAM, KAI +4"). The core places the names (listFlowTwoLines); this measures the pieces and draws.
 */
@Composable
private fun SenderFlow(senders: List<ListSender>, modifier: Modifier = Modifier, startInset: Int = 20, endInset: Int = 20) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val style = BarStyle.copy(fontSize = BarSmallFont)
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val room = with(density) { (maxWidth - startInset.dp - endInset.dp).coerceAtLeast(0.dp).toPx() }
        val lines = remember(senders, room, style, density) {
            fun width(value: String) = measurer.measure(value, style, maxLines = 1, softWrap = false).size.width.toFloat()
            val flow = listSenderFlow(senders, Words)
            listFlowTwoLines(flow, ListFlowWidths(width(flow.prefix), flow.tokens.map(::width), width(flow.separator),
                width(flow.comma), flow.more.map(::width)), room)
        }
        val label = senders.joinToString { senderLabel(it) }
        Column(
            Modifier.fillMaxWidth().padding(start = startInset.dp, end = endInset.dp)
                .clearAndSetSemantics { contentDescription = label },
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            lines.filter { it.isNotEmpty() }.forEach { Text(it, style = style, maxLines = 1, softWrap = false) }
        }
    }
}

private fun senderLabel(sender: ListSender) =
    productArrivalLines(sender.count.toLong(), sender.name, Words).first()

/** The first of the core's candidates that fits ("2× Yo from Christopher", "… from Chr…", "2× Yo"). */
private val BarStyle = TextStyle(fontFamily = YoFont, fontSize = BarFont, color = Color.White)

@Composable
private fun BarWords(
    candidates: List<String>,
    modifier: Modifier = Modifier,
    startInset: Int = 20,
    endInset: Int = 20,
) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val style = BarStyle
    // The widest digit, measured once per density/font scale (ten measurements; the measurer's cache holds eight).
    val slot = remember(measurer, density) {
        ('0'..'9').maxOf { measurer.measure(it.toString(), style, maxLines = 1, softWrap = false).size.width }
    }
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val start = startInset.dp
        val end = endInset.dp
        // As iOS's BarRow: the room between both sides, the line in the middle or moved aside as little as it takes
        // (a thumbnail or sign on one side must not cut "Sending Yo!" to "Sending Y…").
        val available = (maxWidth - start - end).coerceAtLeast(0.dp)
        fun width(value: String) = measurer.measure(value, style, maxLines = 1, softWrap = false).size.width
        fun parts(value: String): Triple<String, String, String> {
            val count = CountDigits.find(value) ?: return Triple(value, "", "")
            return Triple(value.substring(0, count.range.first), count.value, value.substring(count.range.last + 1))
        }
        fun digitBlock(digits: String) = if (digits.isEmpty()) 0 else slot * maxOf(2, digits.length)
        fun total(value: String) = with(density) {
            val (prefix, digits, suffix) = parts(value)
            (width(prefix) + digitBlock(digits) + width(suffix)).toDp()
        }
        val selected = candidates.firstOrNull { total(it) <= available } ?: candidates.last()
        val (prefix, digits, suffix) = parts(selected)
        val lineWidth = total(selected).coerceAtMost(available)
        val lineStart = ((maxWidth - lineWidth) / 2).coerceAtLeast(start).coerceAtMost(maxWidth - end - lineWidth)
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
