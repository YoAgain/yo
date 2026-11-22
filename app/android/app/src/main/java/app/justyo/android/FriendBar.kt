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
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.*
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.changedToUpIgnoreConsumed
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalView
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
import androidx.compose.ui.text.TextMeasurer
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
import uniffi.yo.listTitleWords
import uniffi.yo.listWrapTwoLines
import uniffi.yo.productArrivalLines
import uniffi.yo.productCounted
import uniffi.yo.productMediaSign
import uniffi.yo.productSendWords

internal val BarHeight = 89.dp
private val BarFont = 36.sp
/** A group's senders, two lines in one bar: the second, fixed size (iOS's too: 24 of 36 in an 89 bar). */
private val BarSmallFont = 24.sp
/** The least a small bar line shrinks to (fittedScales), at any text size. */
private val SmallestBarFont = 7.dp
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

/** What a tap on a bar does (the list's `tap`). A request's tap is its yes, a plain Yo, whatever media wait to go
 *  (picked, shared): they stay for the next name, as on iOS. Shared media go to nobody blocked or gone. */
internal enum class BarTap { Nothing, Accept, StageShares, StagePicked, TakePhoto, Retry, Yo }

internal fun barTap(
    request: Boolean, blocked: Boolean, gone: Boolean, shares: Boolean, picked: Boolean, camera: Boolean, unsent: Boolean,
): BarTap = when {
    shares && (blocked || gone) -> BarTap.Nothing
    request -> BarTap.Accept
    shares -> BarTap.StageShares
    picked -> BarTap.StagePicked
    camera -> BarTap.TakePhoto
    unsent -> BarTap.Retry
    else -> BarTap.Yo
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
    /** Media for it waits after a failed try (no connection, refused): the core's "Not Sent · Tap to Retry", as on
     *  iOS, not "Sending" for as long as it waits. */
    val unsent = undo == null && row?.unsent == true
    val sent = undo != null || row?.title == copy(uniffi.yo.ProductText.SENT_YO) || (thumbnails.isNotEmpty() && !unsent)
    val lines = if (friend.request) 2 else 1
}

@Composable internal fun FriendBar(
    repo: Repository, bar: BarContent, color: Color, camera: Boolean, left: Boolean,
    onPress: (Boolean) -> Unit, onTap: () -> Unit, onMenu: () -> Unit, onUndo: () -> Unit,
    onMedia: () -> Unit, onDecline: () -> Unit, onHold: ((Boolean) -> Unit)?,
    onMediaTouch: (MediaTouch) -> Unit = {},
    onHoldDrag: (y: Float, barTop: Float, barBottom: Float, height: Float) -> Unit = { _, _, _, _ -> },
    inControls: (windowY: Float) -> Boolean = { false },
) {
    val friend = bar.friend
    val resources = LocalResources.current
    val tapLabel = stringResource(if (friend.request) R.string.accept_and_yo else R.string.send_yo_action)
    val menuLabel = stringResource(R.string.contact_actions)
    val currentPress by rememberUpdatedState(onPress)
    val currentTap by rememberUpdatedState(onTap)
    val currentHold by rememberUpdatedState(onHold)
    val currentHoldDrag by rememberUpdatedState(onHoldDrag)
    // Where the bar is in the window: a filming finger zooms by how far it is out of it.
    var barTop by remember(friend.key) { mutableFloatStateOf(0f) }
    val root = LocalView.current
    val currentMedia by rememberUpdatedState(onMedia)
    val currentMediaTouch by rememberUpdatedState(onMediaTouch)
    val gesture = if (onHold != null) Modifier.onGloballyPositioned { barTop = it.positionInWindow().y }.pointerInput(friend.key) {
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
                        // Filming, the same finger drawn out of the bar, down or up, zooms the camera (window dp).
                        if (change != null && change.pressed) currentHoldDrag(
                            (barTop + change.position.y) / density, barTop / density,
                            (barTop + size.height) / density, root.rootView.height / density,
                        )
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
        bar.undo != null || (bar.thumbnails.isNotEmpty() && !bar.unsent) -> productSendWords(productCounted(count), true, Words)
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
            // Nothing downloaded yet: shown loading, and a finger on it opens nothing (MediaQueue.openable).
            val loading = bar.media.none { it.ready }
            // A sign, not words: its size is the 40 dp box's at any text size (large text had it over the line).
            Text(sign, Modifier.alpha(if (loading) .35f else 1f), style = TextStyle(fontSize = with(LocalDensity.current) { 26.dp.toSp() }),
                maxLines = 1, softWrap = false)
            if (loading) CircularProgressIndicator(Modifier.size(18.dp), color = Color.White, strokeWidth = 2.dp)
        }
    }
    val currentInControls by rememberUpdatedState(inControls)
    var rowTop by remember(friend.key) { mutableFloatStateOf(0f) }
    Column(Modifier.fillMaxWidth().onGloballyPositioned { rowTop = it.positionInWindow().y }.pointerInput(friend.key) {
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            // In camera mode a finger in the controls' strip, beside them, is for the controls: no Yo, no video.
            // Only its down and up are taken, so the list still scrolls from there.
            if (currentInControls(rowTop + down.position.y)) {
                down.consume()
                do {
                    val event = awaitPointerEvent(PointerEventPass.Initial)
                    event.changes.forEach { if (it.changedToUpIgnoreConsumed()) it.consume() }
                } while (event.changes.any { it.pressed })
                return@awaitEachGesture
            }
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
            if (primaryReceived) ArrivalWords(
                senders ?: listOf(ListSender("", from, receivedCount.coerceIn(0L, UInt.MAX_VALUE.toLong()).toUInt())),
                startInset = if (left) primarySignInset else outgoingInset,
                endInset = if (left) outgoingInset else primarySignInset,
                modifier = receivedModifier,
            ) else BarWords(
                listOf(title),
                startInset = if (left) primarySignInset else outgoingInset,
                endInset = if (left) outgoingInset else primarySignInset,
                // Gone: still their name (whose bar it is), faded, as iOS; a tap says "Not on Yo".
                modifier = if (friend.gone && !bar.sent && bar.undo == null) Modifier.graphicsLayer { alpha = 0.55f } else receivedModifier,
            )
            if (showUndo || thumbnails.isNotEmpty()) Box(Modifier.align(start).padding(horizontal = 20.dp)) {
                OutgoingThumbnail(repo, thumbnails, bar.undo, onUndo)
            }
            if (showSignOnPrimary) Box(Modifier.align(end)) { mediaSign() }
        }
        if (friend.request) {
            Row(Modifier.fillMaxWidth().height(BarHeight)) {
                // Spoken as iOS says them: "Accept and Yo" and "Decline", not the bare words on the bar.
                val accept = stringResource(R.string.accept_and_yo)
                if (left) RequestChoice("Yo", accept, color, Modifier.weight(2f), onTap)
                RequestChoice(stringResource(R.string.ui_no), stringResource(R.string.decline_request), DeclineRed, Modifier.weight(1f), onDecline)
                if (!left) RequestChoice("Yo", accept, color, Modifier.weight(2f), onTap)
            }
        }
    }
}

@Composable private fun RequestChoice(text: String, spoken: String, color: Color, modifier: Modifier, onTap: () -> Unit) {
    Box(modifier.fillMaxHeight().background(color).clickable(
        interactionSource = null, indication = null, role = Role.Button, onClick = onTap,
    ).clearAndSetSemantics { contentDescription = spoken; role = Role.Button; onClick { onTap(); true } },
        contentAlignment = Alignment.Center) { BarWords(text) }
}

private val CountDigits = Regex("[0-9]+(?=×)")

/**
 * Widths of bar words, shared by every bar and kept: each bar measured its words again in each composition with its own
 * measurer (eight entries), and a new bar measured all ten digits first. One measure is 0.3-1 ms of Montserrat on the
 * A34; a Yo's frame measured a dozen (3-4 ms per changed bar). Main thread only, as composition is.
 */
internal class BarMeasure(private val measurer: TextMeasurer) {
    private val sizes = object : LinkedHashMap<Pair<String, TextStyle>, IntSize>(64, .75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Pair<String, TextStyle>, IntSize>) = size > 512
    }
    fun size(text: String, style: TextStyle): IntSize = sizes.getOrPut(text to style) {
        measurer.measure(text, style, maxLines = 1, softWrap = false).size
    }
    fun width(text: String, style: TextStyle): Int = size(text, style).width
    /** The widest digit: a count's steady block. */
    val slot by lazy { ('0'..'9').maxOf { width(it.toString(), BarStyle) } }
}

internal val LocalBarMeasure = staticCompositionLocalOf<BarMeasure?> { null }

/** The list's shared one (FriendList provides it; per density and font scale), else this bar's own. */
@Composable internal fun rememberBarMeasure(): BarMeasure {
    LocalBarMeasure.current?.let { return it }
    val measurer = rememberTextMeasurer(0)
    return remember(measurer) { BarMeasure(measurer) }
}

@Composable
private fun BarWords(text: String, modifier: Modifier = Modifier, startInset: Int = 20, endInset: Int = 20) =
    BarWords(listOf(text), modifier, startInset, endInset)

/**
 * One sender's Yos ("3× Yo from MIA") at the bar's size when the whole line fits, else at the small size over one or
 * two lines (SenderFlow, "3× Yo from" / "MAXIMILIAN_MUELL"); two or more senders always at the small size. Never cut.
 */
@Composable
private fun ArrivalWords(senders: List<ListSender>, modifier: Modifier = Modifier, startInset: Int = 20, endInset: Int = 20) {
    val measure = rememberBarMeasure()
    val density = LocalDensity.current
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val one = senders.singleOrNull()
        val line = one?.let { productArrivalLines(it.count.toLong(), it.name, Words).first() }
        // The digits' steady blocks take a little more than the digits themselves.
        val fits = line != null && with(density) {
            measure.width(line, BarStyle).toDp() + 16.dp
        } <= maxWidth - startInset.dp - endInset.dp
        if (fits) BarWords(listOf(line!!), startInset = startInset, endInset = endInset)
        else SenderFlow(senders, startInset = startInset, endInset = endInset)
    }
}

/**
 * A bar's senders, newest first, over two lines at the small size ("2× Yo from TOM, 3× ANNA," / "4× PETER, LUKAS +2").
 * The core places the names (listFlowTwoLines); this measures the pieces and draws.
 */
@Composable
private fun SenderFlow(senders: List<ListSender>, modifier: Modifier = Modifier, startInset: Int = 20, endInset: Int = 20) {
    val measure = rememberBarMeasure()
    val density = LocalDensity.current
    val style = BarStyle.copy(fontSize = BarSmallFont)
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val room = with(density) { (maxWidth - startInset.dp - endInset.dp).coerceAtLeast(0.dp).toPx() }
        val lines = remember(senders, room, style, density) {
            fun width(value: String) = measure.width(value, style).toFloat()
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
            // Only a name wider than the room even alone (the core places it whole): that line shrinks to fit.
            val shown = lines.filter { it.isNotEmpty() }
            shown.zip(fittedScales(shown, room, style)) { line, scale ->
                Text(line, style = if (scale < 1f) style.copy(fontSize = BarSmallFont * scale) else style,
                    maxLines = 1, softWrap = false)
            }
        }
    }
}

private fun senderLabel(sender: ListSender) =
    productArrivalLines(sender.count.toLong(), sender.name, Words).first()

/** A title at the small size: one line if it fits, else two in whole words (listWrapTwoLines); never cut. */
@Composable
private fun SmallTitle(text: String, room: Float, start: androidx.compose.ui.unit.Dp, end: androidx.compose.ui.unit.Dp) {
    val measure = rememberBarMeasure()
    val style = BarStyle.copy(fontSize = BarSmallFont)
    fun width(value: String) = measure.width(value, style).toFloat()
    val lines = remember(text, room) {
        if (width(text) <= room) listOf(text) else {
            val words = listTitleWords(text)
            listWrapTwoLines(words, words.map(::width), width(" "), room).filter { it.isNotEmpty() }
        }
    }
    Column(
        Modifier.fillMaxWidth().padding(start = start, end = end).clearAndSetSemantics { contentDescription = text },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        // A single word wider than the room: that line shrinks to fit.
        lines.zip(fittedScales(lines, room, style)) { line, scale ->
            Text(line, style = if (scale < 1f) style.copy(fontSize = BarSmallFont * scale) else style,
                maxLines = 1, softWrap = false)
        }
    }
}

/**
 * The small lines' sizes: a line wider than the room shrinks to fit it, and all of them shrink together where they are
 * taller than the bar (large text in the system settings, scripts with tall letters such as Arabic). Never below
 * [SmallestBarFont], which still fits a group's longest name ("#WWW…", 32 bytes) at the largest display and text
 * size. Never cut.
 */
@Composable
private fun fittedScales(lines: List<String>, room: Float, style: TextStyle): List<Float> {
    val measure = rememberBarMeasure()
    val density = LocalDensity.current
    val tall = with(density) { BarHeight.toPx() - 2.dp.toPx() * (lines.size - 1).coerceAtLeast(0) }
    val height = lines.sumOf { measure.size(it, style).height }.toFloat()
    val fit = if (height > tall) tall / height else 1f
    // In sp: large text scales small sizes more than large ones (Android 14's nonlinear font scaling).
    val floor = with(density) { SmallestBarFont.toSp().value / style.fontSize.value }
    return lines.map { line ->
        val width = measure.width(line, style)
        val scale = minOf(if (width > room) room / width else 1f, fit).coerceAtLeast(floor)
        // Not proportional (nonlinear font scaling, the glyphs' rounding): measured again at the size found.
        val scaled = if (scale < 1f) measure.width(line, style.copy(fontSize = style.fontSize * scale)) else 0
        if (scaled > room) (scale * room / scaled).coerceAtLeast(floor) else scale
    }
}

/** The first of the core's candidates that fits ("2× Yo from Christopher", "… from Chr…", "2× Yo"). */
private val BarStyle = TextStyle(fontFamily = YoFont, fontSize = BarFont, color = Color.White)

@Composable
private fun BarWords(
    candidates: List<String>,
    modifier: Modifier = Modifier,
    startInset: Int = 20,
    endInset: Int = 20,
) {
    val measure = rememberBarMeasure()
    val density = LocalDensity.current
    val style = BarStyle
    val slot = measure.slot
    BoxWithConstraints(modifier.fillMaxWidth()) {
        val start = startInset.dp
        val end = endInset.dp
        // As iOS's BarRow: the room between both sides, the line in the middle or moved aside as little as it takes
        // (a thumbnail or sign on one side must not cut "Sending Yo!" to "Sending Y…").
        val available = (maxWidth - start - end).coerceAtLeast(0.dp)
        fun width(value: String) = measure.width(value, style)
        fun parts(value: String): Triple<String, String, String> {
            val count = CountDigits.find(value) ?: return Triple(value, "", "")
            return Triple(value.substring(0, count.range.first), count.value, value.substring(count.range.last + 1))
        }
        fun digitBlock(digits: String) = slot * digits.length  // as iOS's SteadyText: as wide as its digits, no more
        fun total(value: String) = with(density) {
            val (prefix, digits, suffix) = parts(value)
            (width(prefix) + digitBlock(digits) + width(suffix)).toDp()
        }
        val single = candidates.singleOrNull()
        // One title (a name, "Yo Request from …", a send's words) is never cut: too wide at the bar's size, the small
        // size, over two lines in whole words where it must be (as iOS's TitleWords).
        if (single != null && total(single) > available) {
            SmallTitle(single, with(density) { available.toPx() }, start, end)
            return@BoxWithConstraints
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
