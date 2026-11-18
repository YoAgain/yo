@file:OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class, androidx.compose.foundation.ExperimentalFoundationApi::class)
package app.justyo.android

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.SystemClock
import android.view.HapticFeedbackConstants
import androidx.activity.compose.PredictiveBackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.view.PreviewView
import androidx.compose.animation.*
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.*
import uniffi.yo.usernameCanonical
import uniffi.yo.usernameFree

@Composable fun YoRoot(model: YoViewModel) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val repo=model.repository
    val state by repo.state.collectAsStateWithLifecycle()
    val snackbar=remember { SnackbarHostState() }
    LaunchedEffect(state.error,state.notice) {
        val message=if(state.broken) null else state.error ?: state.notice
        if(message!=null) { snackbar.showSnackbar(message,withDismissAction=true);repo.clearMessage(message) }
    }
    Box(Modifier.fillMaxSize()) {
    Surface(Modifier.fillMaxSize(),color=Amethyst,contentColor=Color.White) {
        when {
            state.update -> Column(Modifier.fillMaxSize().safeDrawingPadding().padding(32.dp),verticalArrangement=Arrangement.Center,horizontalAlignment=Alignment.CenterHorizontally) {
                BrandText("Yo",96); Spacer(Modifier.height(24.dp)); BrandText(stringResource(R.string.ui_update_yo),30)
                Text(stringResource(R.string.ui_this_version_of_yo_no_longer_works_install_a_newer_build_to_),textAlign=TextAlign.Center,modifier=Modifier.padding(top=20.dp))
            }
            state.loading -> Box(Modifier.fillMaxSize(),contentAlignment=Alignment.Center) { LoadingIndicator() }
            state.broken -> RecoveryRequired(model)
            state.username==null -> Onboarding(model,state)
            else -> FriendList(model,state)
        }
    }
    state.deletion?.let { deletion ->
        Snackbar(Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(16.dp),action={TextButton(onClick=repo::undoRemove){Text(stringResource(R.string.undo_remove))}}) {
            Text(stringResource(R.string.removed_name,deletion.name))
        }
    }
    SnackbarHost(snackbar,Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(16.dp))
    }
    if(state.firstYo) AlertDialog(onDismissRequest=repo::clearError,title={Text(stringResource(R.string.ui_first_yo_sent))},text={Text(uiResources.getString(R.string.first_sent_explanation ,state.username.orEmpty()))},confirmButton={TextButton(onClick=repo::clearError){Text(stringResource(R.string.ui_awesome))}})
}
@Composable fun BrandText(text: String,size: Int,modifier: Modifier=Modifier) {
    Text(text,modifier,fontFamily=YoFont,fontSize=size.sp,color=Color.White,textAlign=TextAlign.Center)
}
@Composable private fun RecoveryRequired(model: YoViewModel) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current
    val recoveryState by model.repository.state.collectAsStateWithLifecycle()
    Column(Modifier.fillMaxSize().safeDrawingPadding().padding(28.dp),verticalArrangement=Arrangement.Center) {
        BrandText(stringResource(R.string.ui_can_t_open_your_account),28)
        Text(uiResources.getString(R.string.saved_state_kept ,recoveryState.error.orEmpty()),modifier=Modifier.padding(vertical=24.dp))
        Button(onClick={model.repository.command { signOut() }},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_sign_out_and_use_recovery_key))}
    }
}
@Composable private fun Onboarding(model: YoViewModel,s: AppState) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    var step by rememberSaveable { mutableIntStateOf(0) }
    var name by rememberSaveable { mutableStateOf("") }
    var scanning by remember { mutableStateOf(false) }
    var code by rememberSaveable { mutableStateOf("") }
    var free by remember { mutableStateOf<Boolean?>(null) }
    var hint by remember { mutableStateOf<String?>(null) }
    val availability=remember { mutableMapOf<String,Boolean>() }
    val context=LocalContext.current
    val scope=rememberCoroutineScope()
    PredictiveBackHandler(step!=0) { events -> events.collect { };step=0 }
    LaunchedEffect(name) {
        free=availability[name]; hint=null
        if(name.isBlank() || free!=null) return@LaunchedEffect
        delay(350)
        try {
            val result=withContext(Dispatchers.IO) { usernameFree(model.repository.transport,name) }
            ensureActive(); availability[name]=result; free=result
        }
        catch (e: CancellationException) { throw e }
        catch (_: Exception) { hint=uiResources.getString(R.string.ui_choose_a_longer_name_or_try_again_when_connected) }
    }
    Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding().padding(horizontal=24.dp),horizontalAlignment=Alignment.CenterHorizontally) {
        run {
            YoTutorial(Modifier.weight(1f))
            s.pendingInvite?.let { Text(uiResources.getString(R.string.invited_you ,it),modifier=Modifier.padding(bottom=16.dp)) }
            NameEntry(name,{name=usernameCanonical(it)},free)
            hint?.let { Text(it) }
            Button(onClick={model.repository.command { register(name) }},enabled=!s.busy && name.isNotBlank() && free!=false,modifier=Modifier.fillMaxWidth().heightIn(min=54.dp)){BrandButtonText(if(s.busy) uiResources.getString(R.string.ui_creating) else uiResources.getString(R.string.ui_sign_up))}
            Row(Modifier.padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.got_yo), fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
                TextButton(onClick = { step = 2 }) { Text(stringResource(R.string.ui_sign_in)) }
            }
        }
    }
    if(step!=0) ModalBottomSheet(onDismissRequest={step=0},containerColor=Amethyst) {
        Column(Modifier.fillMaxWidth().heightIn(max = 640.dp).verticalScroll(rememberScrollState()).imePadding().padding(horizontal=24.dp),horizontalAlignment=Alignment.CenterHorizontally) {
            TopAppBar(title={},navigationIcon={IconButton(onClick={step=0}){Icon(Icons.AutoMirrored.Filled.ArrowBack,uiResources.getString(R.string.ui_back))}},colors=TopAppBarDefaults.topAppBarColors(containerColor=Amethyst))
            BrandText(if(step==2) uiResources.getString(R.string.ui_sign_in) else uiResources.getString(R.string.ui_recovery_key),30,Modifier.padding(top=24.dp,bottom=32.dp))
            when(step) {
                2 -> Text(stringResource(R.string.ui_use_your_passkey_or_recovery_key),textAlign=TextAlign.Center)
                3 -> {
                    Text(stringResource(R.string.ui_paste_type_or_scan_your_recovery_key),modifier=Modifier.padding(bottom=24.dp))
                    OutlinedTextField(code,{code=it},label={Text(stringResource(R.string.ui_label_64_characters))},minLines=3,textStyle=TextStyle(fontFamily=FontFamily.Monospace),modifier=Modifier.fillMaxWidth())
                    val picker=rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
                        if(uri!=null) scope.launch { try { code=readQr(context,uri) ?: error(uiResources.getString(R.string.ui_we_couldn_t_read_a_recovery_key_from_this_photo)) } catch(e: Exception) { model.error(e.message ?: uiResources.getString(R.string.ui_couldn_t_read_photo)) } }
                    }
                    Row {
                        TextButton(onClick={scanning=true}){Text(stringResource(R.string.scan_qr))}
                        TextButton(onClick={picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))}) { Text(stringResource(R.string.ui_scan_from_photos)) }
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
            if(s.busy) CircularProgressIndicator(Modifier.padding(16.dp))
            when(step) {
                2 -> {
                    Button(onClick={scope.launch { try { Passkeys.signIn(context) } catch(e: Exception) { model.error(e.message ?: uiResources.getString(R.string.ui_use_your_recovery_key)) } }},modifier=Modifier.fillMaxWidth().heightIn(min=54.dp)){BrandButtonText(stringResource(R.string.ui_use_passkey))}
                    TextButton(onClick={step=3}){BrandText(stringResource(R.string.ui_use_recovery_key),17)}
                }
                3 -> Button(onClick={model.repository.command { restore(code) }},enabled=!s.busy && code.isNotBlank(),modifier=Modifier.fillMaxWidth().heightIn(min=54.dp)){BrandButtonText(stringResource(R.string.ui_sign_in))}
            }
            Spacer(Modifier.height(24.dp))
        }
    }
    if(scanning) QrScanner(onClose={scanning=false},onCode={code=it.text;scanning=false})
}
/** The same Android tutorial is used on welcome and in Help. */
@Composable internal fun YoTutorial(modifier: Modifier=Modifier, compact: Boolean = false) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val scope=rememberCoroutineScope()
    Column(modifier,horizontalAlignment=Alignment.CenterHorizontally) {
            val slideCount = if (compact) 3 else 4
            val pager=rememberPagerState(initialPage=slideCount * 1000,pageCount={slideCount * 2000})
            val accessibility=LocalContext.current.getSystemService(Context.ACCESSIBILITY_SERVICE) as android.view.accessibility.AccessibilityManager
            var settledAt by remember { mutableLongStateOf(android.os.SystemClock.uptimeMillis()) }
            LaunchedEffect(pager.settledPage) { settledAt=android.os.SystemClock.uptimeMillis() }
            LaunchedEffect(pager) {
                while(true) {
                    delay((4000-(android.os.SystemClock.uptimeMillis()-settledAt)).coerceAtLeast(200))
                    if(!accessibility.isTouchExplorationEnabled && !pager.isScrollInProgress && android.os.SystemClock.uptimeMillis()-settledAt>=4000) {
                        pager.animateScrollToPage(pager.settledPage+1)
                        settledAt=android.os.SystemClock.uptimeMillis()
                    }
                }
            }
            HorizontalPager(pager,Modifier.weight(1f)) { page ->
                Column(Modifier.fillMaxWidth(),horizontalAlignment=Alignment.CenterHorizontally) {
                    if (compact) {
                        val slides = listOf(
                            R.string.tutorial_tap,
                            R.string.tutorial_back,
                            R.string.tutorial_camera,
                        )
                        val slide = page % slideCount
                        Surface(
                            modifier = Modifier.padding(top = 24.dp).width(140.dp),
                            shape = androidx.compose.foundation.shape.RoundedCornerShape(20.dp),
                            color = Color(0xFF202022),
                        ) {
                            Column(Modifier.padding(6.dp)) {
                                val names = listOf("Leo", "Sam", "Noah", "Mia", "YoGPT", "+")
                                names.forEachIndexed { index, name ->
                                    Box(
                                        Modifier.fillMaxWidth().height(28.dp).background(RowColors[index]),
                                        contentAlignment = Alignment.Center,
                                    ) { BrandText(if (index == 2 && slide < 2) stringResource(if (slide == 0) R.string.sent_yo else R.string.ui_yo) else name, 12) }
                                }
                            }
                        }
                        BrandText(stringResource(slides[slide]), 20, Modifier.padding(top = 12.dp))
                    } else when(page%4) {
                        0 -> { BrandText("Yo",140); BrandText(stringResource(R.string.ui_it_s_that_simple),20) }
                        else -> {
                            val titles=listOf(uiResources.getString(R.string.ui_tap_a_name),uiResources.getString(R.string.ui_yo_back),uiResources.getString(R.string.ui_add_friends_by_name))
                            val lines=listOf(uiResources.getString(R.string.ui_that_s_a_yo_nothing_to_type),uiResources.getString(R.string.ui_a_yo_comes_in_tap_their_name_to_yo_back),uiResources.getString(R.string.ui_yogpt_is_there_from_the_start_it_yos_back))
                            // Android's own row rendering, so the tutorial shows this app.
                            Column(Modifier.width(240.dp).padding(bottom=32.dp)) { FlatRow("YoGPT",0,onClick={}); FlatRow(if(page%4==2) uiResources.getString(R.string.ui_yo) else "+",2,onClick={}) }
                            BrandText(titles[page%4-1],28)
                            Text(lines[page%4-1],textAlign=TextAlign.Center,modifier=Modifier.padding(top=12.dp))
                        }
                    }
                }
            }
            Row(Modifier.padding(bottom=if (compact) 0.dp else 24.dp),horizontalArrangement=Arrangement.spacedBy(4.dp)) {
                repeat(slideCount) { i -> Box(Modifier.size(if (compact) 24.dp else 48.dp).clickable { scope.launch { pager.animateScrollToPage(pager.currentPage-pager.currentPage%slideCount+i) } },contentAlignment=Alignment.Center) {
                    Box(Modifier.size(8.dp).background(Color.White.copy(alpha=if(pager.currentPage%slideCount==i) 1f else .4f),CircleShape))
                } }
            }
    }
}

@Composable private fun BrandButtonText(text: String) { Text(text,fontFamily=YoFont,fontSize=20.sp) }
@Composable fun FlatRow(title: String,color: Int,modifier: Modifier=Modifier,onClick: ()->Unit) {
    Box(modifier.fillMaxWidth().heightIn(min=BarHeight).background(RowColors[Math.floorMod(color,8)]).clickable(interactionSource=null,indication=null,onClick=onClick),contentAlignment=Alignment.Center) {
        BrandText(title,36,Modifier.padding(horizontal=20.dp))
    }
}

@Composable private fun FriendList(model: YoViewModel,s: AppState) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val repo=model.repository
    val shares by model.shares.collectAsStateWithLifecycle()
    val importingShares by model.importingShares.collectAsStateWithLifecycle()
    val context=LocalContext.current
    val owner=LocalLifecycleOwner.current
    val view=LocalView.current
    val scope=rememberCoroutineScope()
    var profile by remember { mutableStateOf(false) }
    var add by remember { mutableStateOf(false) }
    var group by remember { mutableStateOf(false) }
    var name by rememberSaveable { mutableStateOf("") }
    var menu by remember { mutableStateOf<Friend?>(null) }
    var declined by remember { mutableStateOf<Friend?>(null) }
    var cameraOn by remember { mutableStateOf(false) }
    var cameraVersion by remember { mutableIntStateOf(0) }
    var hintEpoch by remember { mutableIntStateOf(0) }
    var hint by remember { mutableStateOf(false) }
    var picked by remember { mutableStateOf<Uri?>(null) }
    var viewing by remember { mutableStateOf<List<Received>?>(null) }
    var recording by remember { mutableStateOf<String?>(null) }
    var more by remember { mutableStateOf(false) }
    val camera=remember { YoCamera(context) }
    val permissions=rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { result ->
        cameraOn=result[Manifest.permission.CAMERA]==true
        if(!cameraOn) model.error(uiResources.getString(R.string.ui_allow_camera_access_in_android_settings_to_send_photos))
    }
    val picker=rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { picked=it; hintEpoch++ }
    DisposableEffect(cameraOn) { onDispose { if(cameraOn) camera.close() } }
    LaunchedEffect(cameraOn,hintEpoch,picked) { hint=false; if(cameraOn) { delay(3000); hint=true } }
    LaunchedEffect(cameraOn) { repo.camera(cameraOn) }
    val stacks by repo.media.undo.collectAsStateWithLifecycle()
    val thumbnails by repo.media.thumbnails.collectAsStateWithLifecycle()
    val friendsByKey = s.friends.associateBy { it.key }
    val friends = (s.listRows.keys.mapNotNull(friendsByKey::get) +
        s.friends.filter { it.key !in s.listRows && it.key != s.deletion?.id }).filterNot { it.blocked }
    val mediaByTarget = s.media.groupBy { it.target }
    val bars = friends.associate { friend ->
        friend.key to BarContent(friend, s.listRows[friend.key], stacks[friend.target],
            mediaByTarget[friend.target].orEmpty(), recording == friend.key, thumbnails[friend.target].orEmpty())
    }
    var frozen by remember { mutableStateOf<Map<String, Int>?>(null) }
    var tapped by remember { mutableStateOf(emptySet<String>()) }
    var lastTap by remember { mutableLongStateOf(0L) }
    var fingersDown by remember { mutableStateOf(emptySet<String>()) }
    val visible = frozen?.let { heights ->
        heights.keys.mapNotNull(friendsByKey::get).filter { it.key in bars } +
            friends.filter { it.key !in heights }
    } ?: friends
    val heights = visible.associate { friend ->
        friend.key to (frozen?.get(friend.key)?.takeUnless { friend.key in tapped } ?: bars.getValue(friend.key).lines)
    }
    val tapLock = remember { BarTapLock() }
    val listState=androidx.compose.foundation.lazy.rememberLazyListState()
    SideEffect {
        val atTop = listState.firstVisibleItemIndex == 0 && listState.firstVisibleItemScrollOffset == 0
        if (tapLock.update(heights) && atTop) listState.requestScrollToItem(0)
    }
    LaunchedEffect(lastTap, recording, fingersDown) {
        if (frozen != null && recording == null && fingersDown.isEmpty()) {
            delay((lastTap + 1500 - SystemClock.elapsedRealtime()).coerceAtLeast(0))
            frozen = null
            tapped = emptySet()
        }
    }
    fun touch(friend: Friend): Boolean {
        if (tapLock.locked(friend.key) || recording != null) return false
        if (frozen == null) frozen = heights
        tapped = tapped + friend.key
        lastTap = SystemClock.elapsedRealtime()
        return true
    }
    fun press(friend: Friend, down: Boolean) {
        if (down && !tapLock.locked(friend.key)) {
            if (frozen == null) frozen = heights
            fingersDown = fingersDown + friend.key
            repo.press(friend.key, true)
        } else if (!down && friend.key in fingersDown) {
            fingersDown = fingersDown - friend.key
            lastTap = SystemClock.elapsedRealtime()
            repo.press(friend.key, false)
        }
    }
    PredictiveBackHandler(recording!=null) { events -> events.collect { } }
    PredictiveBackHandler(recording==null && (cameraOn || picked!=null)) { events -> events.collect { };cameraOn=false;picked=null }
    fun tap(friend: Friend) {
        if(importingShares) return
        if(!touch(friend)) return
        if (friend.request) {
            repo.command {
                val session = sessionId
                answer(friend, true)
                if (sessionId == session) send(friend.copy(request = false, undo = false))
            }
            return
        }
        view.performHapticFeedback(if(android.os.Build.VERSION.SDK_INT>=30) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.VIRTUAL_KEY)
        hintEpoch++
        when {
            shares.isNotEmpty() -> model.stage(friend,shares)
            picked!=null -> { model.stage(friend,listOf(picked!!)); picked=null }
            cameraOn -> {
                val session=repo.sessionId
                scope.launch {
                    try { val uri=camera.photo(); model.stage(friend,listOf(uri),true,session) }
                    catch(e: Exception) { model.error(e.message ?: uiResources.getString(R.string.ui_couldn_t_take_photo)) }
                }
            }
            else -> repo.command { send(friend) }
        }
    }
    val currentTap by rememberUpdatedState<(Friend)->Unit>(::tap)
    val listObscured = profile || group || menu != null || declined != null || viewing != null
    LaunchedEffect(listState, listObscured) {
        if (listObscured) {
            repo.viewport(emptyList())
            return@LaunchedEffect
        }
        snapshotFlow { listState.layoutInfo.visibleItemsInfo.filter { item ->
            val info=listState.layoutInfo
            val visible=minOf(item.offset+item.size,info.viewportEndOffset)-maxOf(item.offset,info.viewportStartOffset)
            visible>=item.size*.6f
        }.map { it.key.toString() } }.collect(repo::viewport)
    }
    fun toggleCamera() {
        if (recording != null) return
        if (cameraOn) {
            cameraOn = false
            picked = null
            camera.close()
        } else if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            cameraOn = true
        } else {
            permissions.launch(arrayOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO))
        }
    }
    val edgeCamera by rememberUpdatedState(::toggleCamera)
    val cameraEdgeOnLeft = cameraOn != s.leftHanded
    val gestureDensity = androidx.compose.ui.platform.LocalDensity.current
    val exclusionWidth = with(gestureDensity) { 28.dp.toPx() }
    val exclusionHalfHeight = with(gestureDensity) { 90.dp.toPx() }
    val edgeGesture = Modifier.systemGestureExclusion { coordinates ->
        val width = coordinates.size.width.toFloat()
        val center = coordinates.size.height / 2f
        val start = if (cameraEdgeOnLeft) 0f else (width - exclusionWidth).coerceAtLeast(0f)
        Rect(start, (center - exclusionHalfHeight).coerceAtLeast(0f),
            (start + exclusionWidth).coerceAtMost(width),
            (center + exclusionHalfHeight).coerceAtMost(coordinates.size.height.toFloat()))
    }.pointerInput(cameraEdgeOnLeft, recording) {
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            if (recording != null) return@awaitEachGesture
            val zone = size.width * if (cameraEdgeOnLeft) .25f else .08f
            val atEdge = if (cameraEdgeOnLeft) down.position.x <= zone else down.position.x >= size.width - zone
            if (!atEdge) return@awaitEachGesture
            val direction = if (cameraEdgeOnLeft) 1 else -1
            val velocity = VelocityTracker().apply { addPosition(down.uptimeMillis, down.position) }
            var dragging = false
            while (true) {
                val event = awaitPointerEvent(PointerEventPass.Initial)
                val finger = event.changes.firstOrNull { it.id == down.id } ?: break
                val movement = finger.position - down.position
                val inward = movement.x * direction
                if (!finger.pressed) {
                    if (dragging && (inward > size.width * .4f ||
                            velocity.calculateVelocity().x * direction >= 500.dp.toPx())) {
                        edgeCamera()
                    }
                    break
                }
                if (!dragging && kotlin.math.abs(movement.y) > viewConfiguration.touchSlop &&
                    kotlin.math.abs(movement.y) * 1.5f >= inward) break
                if (inward >= viewConfiguration.touchSlop && inward > kotlin.math.abs(movement.y) * 1.5f) {
                    dragging = true
                }
                if (dragging) finger.consume()
                velocity.addPosition(finger.uptimeMillis, finger.position)
            }
        }
    }
    Box(Modifier.fillMaxSize().safeDrawingPadding().then(edgeGesture)) {
        if(cameraOn) key(cameraVersion) {
            AndroidView(factory={ctx -> PreviewView(ctx).apply { scaleType=PreviewView.ScaleType.FILL_CENTER; camera.bind(owner,this,model::error) }},modifier=Modifier.fillMaxSize())
        }
        if(picked!=null) PhotoUri(picked!!,Modifier.fillMaxSize())
        LazyColumn(Modifier.fillMaxSize(),state=listState,userScrollEnabled=recording==null,contentPadding=PaddingValues(bottom=84.dp)) {
            if(importingShares) item { LinearProgressIndicator(Modifier.fillMaxWidth());Text(stringResource(R.string.preparing_shared_media),modifier=Modifier.padding(20.dp));TextButton(onClick=model::cancelShares){Text(stringResource(R.string.ui_cancel_share))} }
            if(shares.isNotEmpty()) item { Text(uiResources.getQuantityString(R.plurals.shared_items,shares.size,shares.size),modifier=Modifier.padding(20.dp)); TextButton(onClick=model::cancelShares){Text(stringResource(R.string.ui_cancel_share))} }
            items(visible.size,key={visible[it].key}) { index ->
                val friend = visible[index]
                val dismiss = rememberSwipeToDismissBoxState(confirmValueChange={ value ->
                    if(value==SwipeToDismissBoxValue.EndToStart) { repo.requestRemove(friend);false } else true
                })
                SwipeToDismissBox(state=dismiss,enableDismissFromStartToEnd=false,enableDismissFromEndToStart=!cameraOn && recording==null && !friend.request,
                    backgroundContent={Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.error).padding(horizontal=24.dp),contentAlignment=Alignment.CenterEnd) { Icon(Icons.Default.Delete,stringResource(R.string.ui_remove)) }}) {
                    FriendBar(repo,bars.getValue(friend.key),heights.getValue(friend.key),
                        rowShade(visible,index),cameraOn,s.leftHanded,
                        onPress={press(friend,it)},onTap={currentTap(friend)},onMenu={if(!tapLock.locked(friend.key)) menu=friend},
                        onUndo={if(touch(friend)) repo.command { cancelMedia(friend) }},
                        onMedia={if(touch(friend)) viewing=repo.state.value.media.filter { it.target==friend.target }.takeIf { it.isNotEmpty() }},
                        onDecline={if(touch(friend)) declined=friend},
                        onHold=if(cameraOn && picked==null && shares.isEmpty() && !friend.request) { held ->
                            if(held && touch(friend)) {
                                hintEpoch++
                                recording=friend.key
                                repo.recording(friend.key)
                                view.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                                val session=repo.sessionId
                                val pause=scope.launch(start=CoroutineStart.UNDISPATCHED) {
                                    repo.media.pause(friend.target,session)
                                }
                                camera.startVideo { uri ->
                                    repo.command {
                                        try {
                                            pause.join()
                                            if(uri!=null) model.stage(friend,listOf(uri),true,session).join()
                                            else error(uiResources.getString(R.string.ui_couldn_t_record_video))
                                        } finally {
                                            media.resume(friend.target,session)
                                        }
                                    }
                                    lastTap=SystemClock.elapsedRealtime()
                                    recording=null
                                    repo.recording(null)
                                }
                            } else if(!held && recording==friend.key) {
                                lastTap=SystemClock.elapsedRealtime()
                                camera.stopVideo()
                            }
                        } else null)
                }
            }
            if(!cameraOn) {
                item {
                    if(add) Row(Modifier.fillMaxWidth().heightIn(min=BarHeight).background(RowColors[Repository.nextColor(s.friends)]),verticalAlignment=Alignment.CenterVertically) {
                        OutlinedTextField(name,{ name=usernameCanonical(it) },label={Text(stringResource(R.string.ui_name))},singleLine=true,textStyle=TextStyle(fontFamily=YoFont,fontSize=24.sp),modifier=Modifier.weight(1f))
                        IconButton(onClick={val typed=name; repo.command { add(typed) }; name=""; add=false}){Icon(Icons.Default.Check,uiResources.getString(R.string.ui_add_friend))}
                    } else FlatRow("+",Repository.nextColor(s.friends),onClick={add=true})
                }
                val hasContacts = friends.any {
                    it.group == null && !it.request && uniffi.yo.usernameSkeleton(it.name) !in setOf("YOGPT", "PING")
                }
                if (!hasContacts || add) {
                    item { FlatRow(uiResources.getString(R.string.group_row), Repository.nextColor(s.friends) + 1, onClick = { group = true }) }
                    item { FlatRow(uiResources.getString(R.string.ui_invite), Repository.nextColor(s.friends) + 2, onClick = { shareInvite(context, s.username.orEmpty()) }) }
                }
            }
        }
        Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(horizontal=16.dp),horizontalAlignment=Alignment.CenterHorizontally) {
            AnimatedVisibility(hint && cameraOn) { Surface(shape=MaterialTheme.shapes.large,color=MaterialTheme.colorScheme.surface.copy(alpha=.9f)) {
                Text(if(picked==null) uiResources.getString(R.string.ui_tap_a_name_hold_for_video) else uiResources.getString(R.string.ui_tap_a_name_to_send_this_photo),modifier=Modifier.padding(14.dp))
            } }
            Row(Modifier.fillMaxWidth().padding(vertical=8.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.SpaceBetween) {
                val controls: @Composable ()->Unit = {
                    Box {
                        FloatingActionButton(onClick={if(recording!=null) return@FloatingActionButton; if(cameraOn) picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) else more=true},containerColor=MaterialTheme.colorScheme.surface.copy(alpha=.9f)) { Icon(if(cameraOn) Icons.Default.PhotoLibrary else Icons.Default.MoreHoriz,if(cameraOn) uiResources.getString(R.string.ui_photo_library) else uiResources.getString(R.string.ui_more)) }
                        DropdownMenu(more,{more=false}) {
                            DropdownMenuItem(text={Text(stringResource(R.string.ui_invite))},onClick={more=false;shareInvite(context,s.username.orEmpty())})
                            DropdownMenuItem(text={Text(stringResource(R.string.ui_add_group))},onClick={more=false;group=true})
                            DropdownMenuItem(text={Text(if(s.secured) uiResources.getString(R.string.ui_settings) else uiResources.getString(R.string.ui_settings_1))},onClick={more=false;profile=true})
                        }
                    }
                }
                val cameraButton: @Composable ()->Unit = {
                    FloatingActionButton(onClick=::toggleCamera,containerColor=MaterialTheme.colorScheme.surface.copy(alpha=.9f)) { Icon(if(cameraOn) Icons.Default.Close else Icons.Default.CameraAlt,if(cameraOn) uiResources.getString(R.string.ui_close_camera) else uiResources.getString(R.string.ui_camera)) }
                }
                if(s.leftHanded) cameraButton() else controls()
                if(cameraOn) {
                    TextButton(onClick={camera.zoom()},enabled=recording==null) { Text("${camera.zoom.toInt()}×") }
                    IconButton(onClick={camera.flip();cameraVersion++},enabled=recording==null) { Icon(Icons.Default.Cameraswitch,uiResources.getString(R.string.ui_switch_camera)) }
                } else Spacer(Modifier.weight(1f))
                if(s.leftHanded) controls() else cameraButton()
            }
        }
    }
    if (profile) ProfileSheet(model, s, onClose = { profile = false }, onAddFriends = {
        profile = false
        add = true
        scope.launch { listState.animateScrollToItem(visible.size) }
    })
    if(group) GroupSheet(model,s,onClose={group=false})
    menu?.let { friend -> FriendMenu(model,s,friend,onClose={menu=null}) }
    declined?.let { friend -> AlertDialog(onDismissRequest={declined=null;repo.command { answer(friend,false) }},title={Text(uiResources.getString(R.string.block_name ,friend.name))},
        confirmButton={TextButton(onClick={declined=null;repo.command { answer(friend,false,true) }}){Text("Yo")}},
        dismissButton={TextButton(onClick={declined=null;repo.command { answer(friend,false) }}){Text(stringResource(R.string.ui_no))}}) }
    viewing?.let { queue -> MediaViewer(repo,queue,onClose={viewing=null}) }
}

fun shareInvite(context: Context,name: String) {
    context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT,context.getString(R.string.ui_yo_add_me_on_yo,"https://justyo.co/${Uri.encode(name)}")),context.getString(R.string.ui_invite_friends)))
}
@Composable fun PhotoUri(uri: Uri,modifier: Modifier=Modifier) {
    val context=LocalContext.current
    var bitmap by remember(uri) { mutableStateOf<android.graphics.Bitmap?>(null) }
    LaunchedEffect(uri) { bitmap=withContext(Dispatchers.IO) { runCatching { android.graphics.ImageDecoder.decodeBitmap(android.graphics.ImageDecoder.createSource(context.contentResolver,uri)) { d,info,_ ->
        val ratio=minOf(1.0,2048.0/maxOf(info.size.width,info.size.height));d.setTargetSize((info.size.width*ratio).toInt().coerceAtLeast(1),(info.size.height*ratio).toInt().coerceAtLeast(1));d.allocator=android.graphics.ImageDecoder.ALLOCATOR_SOFTWARE
    } }.getOrNull() } }
    bitmap?.let { Image(it.asImageBitmap(),contentDescription=null,modifier=modifier,contentScale=ContentScale.Crop) }
}

@Composable private fun NameEntry(name: String,onChange: (String)->Unit,available: Boolean?) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val measure=androidx.compose.ui.text.rememberTextMeasurer()
    val density=androidx.compose.ui.platform.LocalDensity.current
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val natural=measure.measure(androidx.compose.ui.text.AnnotatedString(name.ifEmpty { " " }),
            style=TextStyle(fontFamily=YoFont,fontSize=34.sp),softWrap=false,maxLines=1).size.width
        val room=with(density) { (maxWidth-80.dp).toPx() }.coerceAtLeast(1f)
        val size=(34f*minOf(1f,room/natural.coerceAtLeast(1))).coerceAtLeast(8f).sp
        OutlinedTextField(name,onChange,label={Text(stringResource(R.string.ui_name))},singleLine=true,
            textStyle=TextStyle(fontFamily=YoFont,fontSize=size,textAlign=TextAlign.Center),
            trailingIcon={available?.let { Icon(if(it) Icons.Default.CheckCircle else Icons.Default.Cancel,if(it) uiResources.getString(R.string.ui_available) else uiResources.getString(R.string.ui_taken),tint=if(it) Color.Green else Color.Red) }},modifier=Modifier.fillMaxWidth())
    }
}
