// The welcome's pages, as on iOS (Intro.swift): the logo, then cards showing the app in a phone frame with the gesture
// to make (res/drawable-nodpi/intro_tap.png is iOS's IntroTap@3x.png). Help & About shows the same cards.
package app.justyo.android

import android.os.SystemClock
import android.view.accessibility.AccessibilityManager
import androidx.compose.animation.core.*
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PagerState
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CameraAlt
import androidx.compose.material.icons.filled.TouchApp
import androidx.compose.material3.Icon
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.lerp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private enum class IntroGesture { TAP, SWIPE }

/** A card: its title and the gesture drawn over the picture (a tap's height as a share of the picture's). */
private class IntroCard(val title: Int, val gesture: IntroGesture, val tapY: Float = 0f)

private val IntroCards = listOf(
    IntroCard(R.string.tutorial_tap, IntroGesture.TAP, 0.456f),  // the "Sent Yo" bar
    IntroCard(R.string.tutorial_camera, IntroGesture.SWIPE),
    IntroCard(R.string.ui_add_friends, IntroGesture.TAP, 0.868f),  // the "+" bar
)

private const val AdvanceMillis = 6000L
private val PageHeight = 360.dp
private val ShotWidth = 190.dp

/** The carousel's whole height: its pages and the dots just under them. */
internal val IntroHeight = PageHeight + 48.dp

/** An endless carousel: [first] (the logo), then the cards. Advances every 6 s, a full interval after the last swipe or
 *  dot tap; not while [paused] (the keyboard is up) nor with TalkBack. */
@Composable internal fun Intro(modifier: Modifier = Modifier, paused: Boolean = false, first: @Composable () -> Unit) {
    val count = IntroCards.size + 1
    val pager = rememberPagerState(initialPage = count * 1000) { count * 2000 }
    AutoAdvance(pager, paused)
    val scope = rememberCoroutineScope()
    val page = pager.currentPage % count
    Column(modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
        HorizontalPager(pager, Modifier.fillMaxWidth().height(PageHeight)) { index ->
            val shown = index % count
            if (shown == 0) Box(Modifier.fillMaxSize().padding(horizontal = 28.dp), contentAlignment = Alignment.Center) { first() }
            else IntroPage(IntroCards[shown - 1])
        }
        // No dots under the logo (iOS).
        val dots by animateFloatAsState(if (page == 0) 0f else 1f, tween(300), label = "dots")
        Dots(count, page, Modifier.padding(top = 4.dp).alpha(dots)) { target ->
            scope.launch { pager.animateScrollToPage(pager.currentPage - page + target) }
        }
    }
}

/** Help & About's card: the three cards on Amethyst, swiped by hand. */
@Composable internal fun HelpHero(modifier: Modifier = Modifier) {
    val pager = rememberPagerState { IntroCards.size }
    val scope = rememberCoroutineScope()
    Surface(modifier.fillMaxWidth(), shape = RoundedCornerShape(26.dp), color = Amethyst, contentColor = Color.White) {
        Column(Modifier.padding(top = 24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            HorizontalPager(pager, Modifier.fillMaxWidth()) { index ->
                Column(Modifier.fillMaxWidth().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    PhoneShot(IntroCards[index])
                    CardTitle(IntroCards[index], Modifier.padding(top = 24.dp))
                }
            }
            Dots(IntroCards.size, pager.currentPage) { target -> scope.launch { pager.animateScrollToPage(target) } }
        }
    }
}

@Composable private fun AutoAdvance(pager: PagerState, paused: Boolean) {
    val accessibility = LocalContext.current.getSystemService(AccessibilityManager::class.java)
    var settledAt by remember { mutableLongStateOf(SystemClock.uptimeMillis()) }
    LaunchedEffect(pager.settledPage, paused) { settledAt = SystemClock.uptimeMillis() }
    LaunchedEffect(pager, paused) {
        if (paused) return@LaunchedEffect
        while (true) {
            delay((AdvanceMillis - (SystemClock.uptimeMillis() - settledAt)).coerceAtLeast(200))
            if (accessibility?.isTouchExplorationEnabled != true && !pager.isScrollInProgress &&
                SystemClock.uptimeMillis() - settledAt >= AdvanceMillis) {
                pager.animateScrollToPage(pager.currentPage + 1, animationSpec = tween(600))
                settledAt = SystemClock.uptimeMillis()
            }
        }
    }
}

@Composable private fun Dots(count: Int, page: Int, modifier: Modifier = Modifier, go: (Int) -> Unit) {
    val res = LocalResources.current
    // One element for TalkBack, "Page 2 of 4" (iOS's page control): the dots were four unlabeled buttons. The pager
    // itself turns pages with TalkBack's scroll actions.
    Row(modifier.clearAndSetSemantics { contentDescription = res.getString(R.string.intro_page_of, page + 1, count) }) {
        repeat(count) { index ->
            val alpha by animateFloatAsState(if (index == page) 1f else 0.4f, tween(200), label = "dot")
            Box(Modifier.size(24.dp, 44.dp).clickable(interactionSource = null, indication = null) { go(index) },
                contentAlignment = Alignment.Center) {
                Box(Modifier.size(8.dp).background(Color.White.copy(alpha = alpha), CircleShape))
            }
        }
    }
}

/** Picture and words when there is room. */
@Composable private fun IntroPage(card: IntroCard) {
    Column(Modifier.fillMaxSize().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center) {
        PhoneShot(card)
        CardTitle(card, Modifier.padding(top = 24.dp))
    }
}

@Composable private fun CardTitle(card: IntroCard, modifier: Modifier) {
    Text(stringResource(card.title), modifier.semantics { heading() },
        style = TextStyle(fontFamily = YoFont, fontSize = 28.sp, lineHeight = 34.sp, color = Color.White, textAlign = TextAlign.Center))
}

/** The app's picture in the top of a phone frame, fading out below, the gesture drawn over it. [zoom] grows all of it. */
@Composable private fun PhoneShot(card: IntroCard, zoom: Float = 1f) {
    val height = ShotWidth * 743f / 540f
    Box(Modifier.size((ShotWidth + 12.dp) * zoom, (height + 6.dp) * zoom).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        Box(Modifier.requiredSize(ShotWidth + 12.dp, height + 6.dp)
            .graphicsLayer { scaleX = zoom; scaleY = zoom; compositingStrategy = CompositingStrategy.Offscreen }
            .drawWithContent {
                drawContent()
                drawRect(Brush.verticalGradient(0.8f to Color.Black, 1f to Color.Transparent), blendMode = BlendMode.DstIn)
            }
            .background(Color.Black, RoundedCornerShape(topStart = 32.dp, topEnd = 32.dp))
            .padding(start = 6.dp, end = 6.dp, top = 6.dp)) {
            Box(Modifier.size(ShotWidth, height).clip(RoundedCornerShape(topStart = 26.dp, topEnd = 26.dp))) {
                Image(painterResource(R.drawable.intro_tap), null, Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
                when (card.gesture) {
                    IntroGesture.TAP -> TapHint(card.tapY, ShotWidth, height)
                    IntroGesture.SWIPE -> SwipeHint(ShotWidth, height)
                }
            }
        }
    }
}

/** Still for the first part of each round, then [curve] over the rest; then it starts again. */
private fun delayed(rest: Float, curve: Easing) = Easing { f -> if (f < rest) 0f else curve.transform((f - rest) / (1f - rest)) }

/** A finger taps the bar: a ring grows and fades under it. */
@Composable private fun TapHint(y: Float, width: Dp, height: Dp) {
    val progress by rememberInfiniteTransition("tap").animateFloat(0f, 1f,
        infiniteRepeatable(tween(1200, easing = delayed(0.25f, EaseOut))), label = "tap")
    val ring = lerp(18.dp, 64.dp, progress)
    val x = width * 0.5f
    val top = height * y
    Box(Modifier.offset { IntOffset((x - ring / 2).roundToPx(), (top - ring / 2).roundToPx()) }.size(ring)
        .border(3.dp, Color.White.copy(alpha = 0.8f * (1f - progress)), CircleShape))
    Icon(Icons.Default.TouchApp, null, Modifier.offset { IntOffset((x - 6.dp).roundToPx(), (top + 6.dp).roundToPx()) }.size(40.dp)
        .graphicsLayer { val scale = 0.86f + 0.14f * progress; scaleX = scale; scaleY = scale }, tint = Color.White)
}

/** A finger swipes in from the edge, the camera following it. */
@Composable private fun SwipeHint(width: Dp, height: Dp) {
    val progress by rememberInfiniteTransition("swipe").animateFloat(0f, 1f,
        infiniteRepeatable(tween(1800, easing = delayed(0.22f, EaseInOut))), label = "swipe")
    Box(Modifier.offset { IntOffset((width * (0.92f - 0.52f * progress)).roundToPx(), 0) }.size(width * (0.08f + 0.52f * progress), height)
        .clipToBounds().background(Color.Black.copy(alpha = 0.35f)), contentAlignment = Alignment.Center) {
        Icon(Icons.Default.CameraAlt, null, Modifier.requiredSize(34.dp), tint = Color.White)
    }
    Icon(Icons.Default.TouchApp, null, Modifier.offset { IntOffset((width * (0.82f - 0.5f * progress)).roundToPx(), (height * 0.45f).roundToPx()) }.size(40.dp), tint = Color.White)
}
