@file:OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class, androidx.compose.foundation.ExperimentalFoundationApi::class)
package app.justyo.android

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.SystemClock
import android.os.Build
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.draw.clip
import androidx.compose.ui.window.DialogWindowProvider
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.rememberTextMeasurer
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
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.flowOn
import uniffi.yo.ProductText
import uniffi.yo.productInvitedYou
import uniffi.yo.usernameCanonical
import uniffi.yo.usernameFree
import uniffi.yo.usernameValid

@Composable fun YoRoot(model: YoViewModel) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val repo=model.repository
    val screen = remember(repo) { repo.state.map { it.forScreen() }.distinctUntilChanged().flowOn(Dispatchers.Default) }
    val first = remember(repo) { repo.state.value.forScreen() }
    val state by screen.collectAsStateWithLifecycle(first)
    val snackbar=remember { SnackbarHostState() }
    LaunchedEffect(state.error,state.notice,state.username,state.broken) {
        val message=if(state.broken || state.username == null) null else state.error ?: state.notice
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
            state.updating && state.username == null -> Column(Modifier.fillMaxSize().safeDrawingPadding().padding(32.dp),verticalArrangement=Arrangement.Center,horizontalAlignment=Alignment.CenterHorizontally) {
                BrandText("Yo",96); Spacer(Modifier.height(24.dp)); LoadingIndicator()
                Text(stringResource(R.string.yo_is_updating),textAlign=TextAlign.Center,modifier=Modifier.padding(top=20.dp))
            }
            state.broken -> RecoveryRequired(model)
            state.username==null -> Onboarding(model,state)
            else -> CompositionLocalProvider(LocalBarMeasure provides rememberBarMeasure()) { FriendList(model,state) }
        }
    }
    state.deletion?.let { deletion ->
        Snackbar(Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(16.dp),action={TextButton(onClick=repo::undoRemove){Text(stringResource(R.string.undo_remove))}}) {
            Text(stringResource(R.string.removed_name,deletion.name))
        }
    }
    SnackbarHost(snackbar,Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(16.dp))
    }
    state.invitation?.let { invitation -> YoSystemTheme { AlertDialog(onDismissRequest=repo::dismissInvite,title={Text(invitation.name)},
        text={Text(invitation.problem ?: copy(if(invitation.known) ProductText.INVITE_YO else ProductText.INVITE_ADD_YO))},
        confirmButton={
            if (invitation.problem != null) TextButton(onClick=repo::dismissInvite){Text(stringResource(android.R.string.ok))}
            else TextButton(onClick={repo.command { acceptInvite(invitation) }}){Text(copy(if(invitation.known) ProductText.YO else ProductText.ADD_AND_YO))}
        },
        dismissButton=if (invitation.problem != null) null else {{TextButton(onClick=repo::dismissInvite){Text(copy(ProductText.CANCEL))}}}) } }
    // As iOS: without notifications it offers them, a Yo back would otherwise go unnoticed.
    val notificationsOn by model.notificationsEnabled.collectAsStateWithLifecycle()
    val context = LocalContext.current
    if(state.firstYo) YoSystemTheme { AlertDialog(onDismissRequest=repo::clearError,title={Text(stringResource(R.string.ui_first_yo_sent))},
        text={Text(uiResources.getString(R.string.first_sent_explanation ,state.username.orEmpty()) +
            if (notificationsOn) "" else " " + stringResource(R.string.first_sent_turn_on))},
        confirmButton={
            if (notificationsOn) TextButton(onClick=repo::clearError){Text(stringResource(R.string.ui_awesome))}
            else TextButton(onClick={ repo.clearError(); openNotificationSettings(context) }){Text(copy(ProductText.TURN_ON_NOTIFICATIONS))}
        },
        dismissButton=if (notificationsOn) null else {{ TextButton(onClick=repo::clearError){Text(stringResource(R.string.not_now))} }}) }
    state.emptyGroup?.let { group -> YoSystemTheme { AlertDialog(onDismissRequest=repo::emptyGroupSeen,
        title={Text(copy(ProductText.GROUP_EMPTY))},
        text={Text(uiResources.getString(R.string.group_empty_delete, group.name))},
        confirmButton={TextButton(onClick={ repo.command { deleteEmptyGroup(group) } }){Text(stringResource(R.string.delete_group))}},
        dismissButton={TextButton(onClick=repo::emptyGroupSeen){Text(stringResource(R.string.ui_cancel))}}) } }
    state.keyChanged?.let { change -> YoSystemTheme { AlertDialog(onDismissRequest=repo::keyChangeSeen,
        title={Text(uiResources.getString(R.string.has_a_new_key, change.friend.name))},
        text={Text(uiResources.getString(R.string.new_key_explanation, change.friend.name))},
        confirmButton={
            if (change.confirm) TextButton(onClick={ repo.keyChangeSeen(); repo.command { confirm(change.friend) } }){Text(stringResource(R.string.confirm_new_key))}
            else TextButton(onClick=repo::keyChangeSeen){Text(stringResource(android.R.string.ok))}
        },
        dismissButton=if (change.confirm) {{ TextButton(onClick=repo::keyChangeSeen){Text(stringResource(android.R.string.ok))} }} else null) } }
}
@Composable fun BrandText(text: String, size: Int, modifier: Modifier = Modifier, maxLines: Int = Int.MAX_VALUE) {
    Text(text, modifier, maxLines = maxLines, softWrap = maxLines > 1,
        style = TextStyle(fontFamily = YoFont, fontSize = size.sp, color = Color.White, textAlign = TextAlign.Center))
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
    var code by rememberSaveable { mutableStateOf("") }
    val availability = remember { mutableStateMapOf<String, Boolean>() }
    val free = availability[name]
    val valid = remember(name) { usernameValid(name) }
    // How short a name may be only the hub knows (spec 3.3): its 400 marks a name too short.
    val hubShortNames = remember { mutableStateMapOf<String, Boolean>() }
    val rejectedAsShort = hubShortNames[name] == true
    var tooShort by remember { mutableStateOf(false) }
    val nameFocus = remember { FocusRequester() }
    val shake = remember { androidx.compose.animation.core.Animatable(0f) }
    var hint by remember { mutableStateOf<String?>(null) }
    val context=LocalContext.current
    val scope=rememberCoroutineScope()
    val focusManager = LocalFocusManager.current
    val keyboardVisible = WindowInsets.ime.getBottom(LocalDensity.current) > 0
    val repo = model.repository
    val canRegister = !s.busy && free != false
    val register: () -> Unit = {
        if (canRegister && !valid) {
            nameFocus.requestFocus()
            tooShort = name.isNotEmpty()
            scope.launch {
                for (position in listOf(-12f, 10f, -7f, 4f, 0f)) {
                    shake.animateTo(position, androidx.compose.animation.core.tween(70))
                }
            }
        } else if (canRegister) {
            focusManager.clearFocus()
            val submitted = name
            repo.clearError()
            repo.command { register(submitted) }
        }
    }
    LaunchedEffect(name, rejectedAsShort) {
        tooShort = false
        if (rejectedAsShort) {
            delay(1500)
            tooShort = true
        }
    }
    LaunchedEffect(name) {
        hint=null
        if(!valid || free!=null) return@LaunchedEffect
        delay(500)
        try {
            val result=withContext(Dispatchers.IO) { usernameFree(model.repository.transport,name) }
            ensureActive(); availability[name]=result
        }
        catch (e: CancellationException) { throw e }
        catch (failure: Exception) {
            if (failure is uniffi.yo.YoException.Refused && failure.status.toInt() == 400) {
                hubShortNames[name] = true
            } else {
                hint = copy(ProductText.LONGER_NAME_OFFLINE)
            }
        }
    }
    Column(Modifier.fillMaxSize().safeDrawingPadding().imePadding().padding(bottom = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        // The carousel in the room above the form, shrunk into it with the keyboard up and held still meanwhile (iOS
        // Onboarding): the form below keeps its own height, never cut.
        BoxWithConstraints(Modifier.fillMaxWidth().weight(1f), contentAlignment = Alignment.Center) {
            val scale = (maxHeight / IntroHeight).coerceAtMost(1f)
            Intro(Modifier.requiredHeight(IntroHeight).graphicsLayer {
                scaleX = scale; scaleY = scale; alpha = if (scale < 0.3f) 0f else 1f
            }, paused = keyboardVisible) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    BrandText("Yo", 150, maxLines = 1)
                    BrandText(copy(ProductText.THAT_SIMPLE), 20, Modifier.alpha(0.85f), maxLines = 1)
                }
            }
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            s.pendingInvite?.let { Text(productInvitedYou(it, Words), Modifier.padding(bottom = 16.dp), style = TextStyle(fontFamily = YoFont, fontSize = 17.sp, color = Color.White.copy(alpha = 0.85f))) }
            Box(Modifier.fillMaxWidth().height(22.dp), contentAlignment = Alignment.Center) {
                if (free == false) {
                    Row(
                        Modifier.clickable { focusManager.clearFocus(); repo.clearError(); step = 2 },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(copy(ProductText.NAME_TAKEN) + "\u00A0", color = Color.White.copy(alpha = 0.8f),
                            fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        Text(copy(ProductText.SIGN_IN), color = Color.White,
                            fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textDecoration = TextDecoration.Underline)
                    }
                } else if (tooShort) {
                    Text(copy(ProductText.LONGER_NAME), fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                }
            }
            Spacer(Modifier.height(8.dp))
            val (nameValue, onName) = nameField(name) { name = it; repo.clearError() }
            NameEntry(nameValue, onName,
                if (tooShort) false else free, Modifier.offset { IntOffset(shake.value.dp.roundToPx(), 0) }.focusRequester(nameFocus), register)
            Spacer(Modifier.height(10.dp))
            OnboardingButton(
                if (s.busy) stringResource(R.string.ui_creating) else copy(ProductText.SIGN_UP),
                canRegister, register,
            )
            OnboardingNote(s.error.takeIf { step == 0 } ?: hint)
            TextButton(
                onClick = { focusManager.clearFocus(); repo.clearError(); step = 2 },
                modifier = Modifier.padding(top = 14.dp),
                contentPadding = PaddingValues(0.dp),
            ) {
                val question = copy(ProductText.GOT_YO)
                val questionStyle = TextStyle(
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                    color = Color.White.copy(alpha = 0.85f),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(question, Modifier.alignByBaseline(), style = questionStyle, maxLines = 1)
                    Text(
                        copy(ProductText.SIGN_IN),
                        Modifier.alignByBaseline(),
                        style = TextStyle(fontFamily = YoFont, fontSize = 18.sp, color = Color.White),
                        maxLines = 1,
                    )
                    Text(
                        question,
                        Modifier.alignByBaseline().alpha(0f).clearAndSetSemantics {},
                        style = questionStyle,
                        maxLines = 1,
                    )
                }
            }
        }
    }
    if (step == 2) ModalBottomSheet(
        onDismissRequest = { if (step == 2) { repo.clearError(); step = 0 } },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Amethyst,
        contentColor = Color.White,
        dragHandle = null,
        contentWindowInsets = { WindowInsets(0, 0, 0, 0) },
    ) {
        val window = (LocalView.current.parent as? DialogWindowProvider)?.window
        SideEffect {
            if (Build.VERSION.SDK_INT >= 29) window?.isNavigationBarContrastEnforced = false
        }
        SignInCard(model, s, code) { code = it }
    }
}

/** A complete recovery key in what was typed, pasted or read (as `Repository.restore` reads it); none otherwise. */
/** A sign-in option's sign where the account's avatar sits, at the avatar's size. */
@Composable
private fun SignInOptionIcon(icon: androidx.compose.ui.graphics.vector.ImageVector) {
    Box(Modifier.size(36.dp).background(Color.White.copy(alpha = 0.18f), CircleShape), contentAlignment = Alignment.Center) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(20.dp))
    }
}

private fun parsedRecoveryKey(text: String): String? =
    uniffi.yo.recoveryKeyParse(text)
        ?: text.lowercase().filter { it in 'a'..'z' || it in '0'..'9' }.takeIf { it.length >= 64 }
            ?.let { uniffi.yo.recoveryKeyParse(it.takeLast(64)) }

private enum class SignInWay { SAVED, PASSKEY, KEY }

/**
 * The Sign In card (iOS SignInCard, design J): the saved Block Store account as a white capsule, or a notice that there
 * is none; under "Use a Key" the passkey and one recovery key field, which signs in once it holds a whole key.
 */
@Composable
private fun SignInCard(model: YoViewModel, s: AppState, code: String, onCode: (String) -> Unit) {
    val repo = model.repository
    val context = LocalContext.current
    val uiResources = androidx.compose.ui.platform.LocalResources.current
    val scope = rememberCoroutineScope()
    val focusManager = LocalFocusManager.current
    var saved by remember { mutableStateOf<uniffi.yo.SyncedItem?>(null) }
    var looked by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { saved = repo.savedAccount(); looked = true }
    var opened by rememberSaveable { mutableStateOf<Boolean?>(null) }
    val open = opened ?: (looked && saved == null)
    var by by remember { mutableStateOf<SignInWay?>(null) }
    var tried by remember { mutableStateOf<String?>(null) }
    var keyProblem by remember { mutableStateOf<String?>(null) }
    val busy = s.busy || by != null
    val mismatch = stringResource(R.string.ui_that_doesn_t_match_a_recovery_key_check_it_and_try_again)
    // "Remove animations" in the system settings: the options open without motion (iOS: Reduce Motion).
    val still = remember(context) {
        android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    var scanning by remember { mutableStateOf(false) }
    fun start(way: SignInWay, work: suspend () -> Unit) {
        if (busy) return
        by = way; keyProblem = null; repo.clearError()
        scope.launch { try { work() } finally { by = null } }
    }
    fun submit(key: String) {
        tried = key
        focusManager.clearFocus()
        start(SignInWay.KEY) { repo.command { restore(key) }.join() }
    }
    fun submitTyped(text: String) {
        val key = parsedRecoveryKey(text)
        if (key == null) keyProblem = mismatch else submit(key)
    }
    // In the sheet's window, so it opens over the card; a scanned key signs in at once.
    if (scanning) QrScanner(onClose = { scanning = false }, onCode = { onCode(it.text); scanning = false; submitTyped(it.text) })
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) scope.launch {
            val text = runCatching { readQr(context, uri) }.getOrNull()
            if (text == null) keyProblem = uiResources.getString(R.string.ui_we_couldn_t_read_a_recovery_key_from_this_photo)
            else { onCode(text); submitTyped(text) }
        }
    }
    // Many accounts, or the key options opened, can outgrow the screen: the sheet grows to full height at most and the
    // card scrolls inside it; opened, the passkey and the key field come into view.
    val scroll = rememberScrollState()
    LaunchedEffect(open) {
        if (open) { kotlinx.coroutines.delay(400); scroll.animateScrollTo(scroll.maxValue) }
    }
    Column(
        Modifier.fillMaxWidth().navigationBarsPadding().imePadding().verticalScroll(scroll)
            .then(if (still) Modifier else Modifier.animateContentSize())
            .padding(start = 24.dp, end = 24.dp, top = 28.dp, bottom = 12.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        BrandText(copy(ProductText.SIGN_IN), 28)
        val item = saved
        if (item != null) {
            val label = stringResource(R.string.signin_as, item.username)
            Surface(
                onClick = { start(SignInWay.SAVED) { repo.command { signInSaved(item) }.join() } }, enabled = !busy,
                modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp).semantics { contentDescription = label },
                shape = CircleShape, color = Color.White, contentColor = Color.Black,
            ) {
                Row(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    SettingsAvatar(item.username, 36)
                    Spacer(Modifier.width(12.dp))
                    Text(item.username, Modifier.weight(1f), maxLines = 1,
                        style = TextStyle(fontFamily = YoFont, fontSize = 22.sp, color = Color.Black))
                    if (by == SignInWay.SAVED) CircularProgressIndicator(Modifier.size(22.dp), color = Color.Black, strokeWidth = 2.5.dp)
                }
            }
        } else if (looked) {
            Box(Modifier.fillMaxWidth().heightIn(min = 44.dp).background(Color.White.copy(alpha = 0.16f), CircleShape)
                .padding(horizontal = 16.dp, vertical = 10.dp), contentAlignment = Alignment.Center) {
                Text(copy(ProductText.NO_SAVED_ACCOUNT), color = Color.White.copy(alpha = 0.85f), fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
            }
        }
        // As the welcome's "Got Yo? Sign In": the quiet question, then the action.
        val keyLabel = stringResource(R.string.signin_use_a_key)
        TextButton(
            onClick = { opened = !open },
            modifier = Modifier.clearAndSetSemantics {
                contentDescription = keyLabel
                role = androidx.compose.ui.semantics.Role.Button
                onClick { opened = !open; true }
            },
            contentPadding = PaddingValues(0.dp),
        ) {
            val question = copy(ProductText.NOT_LISTED)
            val questionStyle = TextStyle(fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = Color.White.copy(alpha = 0.85f))
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(question, Modifier.alignByBaseline(), style = questionStyle, maxLines = 1)
                Text(copy(ProductText.USE_A_KEY), Modifier.alignByBaseline(),
                    style = TextStyle(fontFamily = YoFont, fontSize = 18.sp, color = Color.White), maxLines = 1)
                Text(question, Modifier.alignByBaseline().alpha(0f), style = questionStyle, maxLines = 1)
            }
        }
        AnimatedVisibility(
            open,
            enter = if (still) EnterTransition.None else fadeIn() + expandVertically(),
            exit = if (still) ExitTransition.None else fadeOut() + shrinkVertically(),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                // The account's capsule, in glass: icon where the avatar sits, the name's size, the same height.
                Surface(
                    onClick = {
                        start(SignInWay.PASSKEY) {
                            try {
                                val assertion = Passkeys.assert(context)
                                repo.command { restorePasskey(assertion) }.join()
                            } catch (cancelled: CancellationException) {
                                throw cancelled
                            } catch (failure: Exception) {
                                model.error(failure.message ?: uiResources.getString(R.string.ui_use_your_recovery_key))
                            }
                        }
                    },
                    // As the key field below it: the passkey and the recovery key are two equal options.
                    enabled = !busy, shape = CircleShape,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp),
                    color = Color.White.copy(alpha = 0.16f), contentColor = Color.White,
                ) {
                    Row(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        SignInOptionIcon(Icons.Default.Key)
                        Spacer(Modifier.width(12.dp))
                        Text(copy(ProductText.PASSKEY), Modifier.weight(1f), maxLines = 1,
                            style = TextStyle(fontFamily = YoFont, fontSize = 22.sp, color = Color.White))
                        if (by == SignInWay.PASSKEY) CircularProgressIndicator(Modifier.size(22.dp), color = Color.White, strokeWidth = 2.5.dp)
                    }
                }
                Row(Modifier.clearAndSetSemantics {}, verticalAlignment = Alignment.CenterVertically) {
                    HorizontalDivider(Modifier.weight(1f), color = Color.White.copy(alpha = 0.4f))
                    Text(copy(ProductText.OR), Modifier.padding(horizontal = 10.dp), color = Color.White.copy(alpha = 0.7f),
                        fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    HorizontalDivider(Modifier.weight(1f), color = Color.White.copy(alpha = 0.4f))
                }
                val placeholder = copy(ProductText.RECOVERY_KEY)
                BasicTextField(
                    value = code,
                    onValueChange = { typed ->
                        onCode(typed); keyProblem = null
                        val key = parsedRecoveryKey(typed)
                        if (key != null && key != tried) submit(key)
                        else if (key == null && typed.count { it.isLetterOrDigit() } >= 64) keyProblem = mismatch
                    },
                    modifier = Modifier.fillMaxWidth().semantics { contentDescription = placeholder },
                    enabled = !busy,
                    singleLine = true,
                    textStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 22.sp,
                        fontWeight = FontWeight.SemiBold, color = Color.White),
                    cursorBrush = SolidColor(Color.White),
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, capitalization = KeyboardCapitalization.None,
                        keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submitTyped(code) }),
                    decorationBox = { field ->
                        Row(Modifier.heightIn(min = 56.dp).background(Color.White.copy(alpha = 0.16f), CircleShape)
                            .padding(start = 12.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            SignInOptionIcon(Icons.Default.VpnKey)
                            Spacer(Modifier.width(12.dp))
                            Box(Modifier.weight(1f)) {
                                if (code.isEmpty()) Text(placeholder, maxLines = 1,
                                    style = TextStyle(fontFamily = YoFont, fontSize = 22.sp, color = Color.White.copy(alpha = 0.45f)))
                                field()
                            }
                            when {
                                by == SignInWay.KEY -> Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
                                    CircularProgressIndicator(Modifier.size(22.dp), color = Color.White, strokeWidth = 2.5.dp)
                                }
                                code.isNotEmpty() -> FilledIconButton(
                                    onClick = { submitTyped(code) }, enabled = !busy, modifier = Modifier.size(40.dp),
                                    colors = IconButtonDefaults.filledIconButtonColors(containerColor = Color.White, contentColor = Color.Black),
                                ) { Icon(Icons.AutoMirrored.Filled.ArrowForward, copy(ProductText.SIGN_IN)) }
                                else -> Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    KeyFieldButton(Icons.Default.PhotoCamera, stringResource(R.string.signin_scan)) { scanning = true }
                                    KeyFieldButton(Icons.Default.Image, stringResource(R.string.signin_photo)) {
                                        picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                                    }
                                }
                            }
                        }
                    },
                )
            }
        }
        OnboardingNote(keyProblem ?: s.error)
    }
}

@Composable
private fun KeyFieldButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, onClick: () -> Unit) {
    FilledTonalIconButton(
        onClick = onClick, modifier = Modifier.size(40.dp),
        colors = IconButtonDefaults.filledTonalIconButtonColors(containerColor = Color.White.copy(alpha = 0.18f), contentColor = Color.White),
    ) { Icon(icon, label, Modifier.size(20.dp)) }
}
@Composable fun FlatRow(title: String,color: Int,modifier: Modifier=Modifier,onClick: ()->Unit) {
    Box(modifier.fillMaxWidth().heightIn(min=BarHeight).background(RowColors[Math.floorMod(color,8)]).clickable(interactionSource=null,indication=null,onClick=onClick),contentAlignment=Alignment.Center) {
        BrandText(title,36,Modifier.padding(horizontal=20.dp))
    }
}

@Composable
private fun ListActionRow(
    label: String,
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    color: Color,
    modifier: Modifier,
    onClick: () -> Unit,
) {
    Surface(onClick = onClick, modifier = modifier.height(BarHeight), color = color, contentColor = Color.White) {
        Row(
            horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(icon, contentDescription = null, modifier = Modifier.size(22.dp))
            Text(label, style = TextStyle(fontFamily = YoFont, fontSize = 22.sp, lineHeight = 26.sp))
        }
    }
}

@Composable
private fun NotificationSettingsRow(onClick: () -> Unit) {
    val label = copy(uniffi.yo.ProductText.TURN_ON_NOTIFICATIONS)
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    BoxWithConstraints(
        Modifier.fillMaxWidth().height(BarHeight).background(Color(0xFFE74C3C)).clickable(onClick = onClick)
            .padding(horizontal = 16.dp),
        contentAlignment = Alignment.Center,
    ) {
        val style = TextStyle(fontFamily = YoFont, fontSize = 36.sp, color = Color.White)
        val naturalWidth = measurer.measure(label, style, softWrap = false, maxLines = 1).size.width
        val room = with(density) { maxWidth.toPx() }
        val scale = (room / naturalWidth.coerceAtLeast(1)).coerceIn(0.5f, 1f)
        Text(label, style = style.copy(fontSize = 36.sp * scale), maxLines = 1)
    }
}

@Composable private fun FriendList(model: YoViewModel,s: AppState) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val repo=model.repository
    val shares by model.shares.collectAsStateWithLifecycle()
    val notificationsEnabled by model.notificationsEnabled.collectAsStateWithLifecycle()
    var notificationInFlight by rememberSaveable { mutableStateOf(false) }
    val notificationPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        notificationInFlight = false
        model.notificationResult(it)
    }
    val importingShares by model.importingShares.collectAsStateWithLifecycle()
    val sharing = importingShares || shares.isNotEmpty()
    androidx.activity.compose.BackHandler(enabled = sharing, onBack = model::cancelShares)
    val context=LocalContext.current
    val notificationPreferences = remember(context) { context.getSharedPreferences("notification-permission", Context.MODE_PRIVATE) }
    var notificationAsked by remember { mutableStateOf(notificationPreferences.getBoolean("asked", false)) }
    LaunchedEffect(notificationsEnabled) {
        if (Build.VERSION.SDK_INT >= 33 && !notificationsEnabled && !notificationAsked && !model.deniedThisLaunch) {
            notificationInFlight = true
            notificationAsked = true
            notificationPreferences.edit { putBoolean("asked", true) }
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
    val showNotificationBar = uniffi.yo.productShowNotificationRow(
        denied = !notificationsEnabled && (Build.VERSION.SDK_INT < 33 || notificationAsked),
        askedThisLaunch = notificationInFlight || model.deniedThisLaunch,
    )

    val keyboardVisible = WindowInsets.ime.getBottom(LocalDensity.current) > 0
    val owner=LocalLifecycleOwner.current
    val view=LocalView.current
    val scope=rememberCoroutineScope()
    // Saveable: a rotation or a process restored by Android keeps Settings, Add Friend and New Group open.
    var profile by rememberSaveable { mutableStateOf(false) }
    var add by rememberSaveable { mutableStateOf(false) }
    var group by rememberSaveable { mutableStateOf(false) }
    var name by rememberSaveable { mutableStateOf("") }
    var addingFriend by remember { mutableStateOf(false) }
    // While the hub is asked: "NAME…" in the empty field, as iOS; the field takes the next name meanwhile.
    var lookingUp by remember { mutableStateOf<String?>(null) }
    var addError by remember { mutableStateOf<String?>(null) }
    val friendFocus = remember { FocusRequester() }
    val focusManager = LocalFocusManager.current
    val canAdd = remember(name, addingFriend) { !addingFriend && usernameValid(name) }
    val submitFriend = {
        if (canAdd) {
            val submitted = name
            addingFriend = true
            addError = null
            lookingUp = "${uniffi.yo.usernameCanonical(submitted)}…"
            name = ""
            focusManager.clearFocus()
            repo.command {
                // The hub not reached or refusing: the banner says why and the name comes back into the field, for
                // another try; nothing is added. This phone's own answers ("No Such User", "That's You") in the field.
                var unreached = false
                val failure = try {
                    add(submitted)
                    null
                } catch (exception: CancellationException) {
                    throw exception
                } catch (exception: Exception) {
                    if (exception is uniffi.yo.YoException.Replaced) throw exception
                    unreached = exception is uniffi.yo.YoException || exception is java.io.IOException
                    if (unreached) Problems.report(exception)
                    friendlyError(context, exception)
                }
                withContext(Dispatchers.Main.immediate) {
                    addingFriend = false
                    lookingUp = null
                    if (unreached) { if (name.isEmpty()) name = submitted; runCatching { friendFocus.requestFocus() } } else addError = failure
                    if (failure == null && name.isEmpty()) add = false
                }
            }
        }
    }

    var menu by remember { mutableStateOf<Friend?>(null) }
    var cameraOn by remember { mutableStateOf(false) }
    var cameraVersion by remember { mutableIntStateOf(0) }
    var hintEpoch by remember { mutableIntStateOf(0) }
    var hint by remember { mutableStateOf(false) }
    // A photo or video went out since the camera opened: its hint is not needed again until the next opening (iOS).
    var hintNeeded by remember { mutableStateOf(true) }
    // Up to ten photos and videos from the library, in order (iOS's picker); a tap on a name sends them all.
    var picked by remember { mutableStateOf<List<Uri>?>(null) }
    var viewing by remember { mutableStateOf<List<Received>?>(null) }
    val heldFinger = remember { HeldFinger() }
    var recording by remember { mutableStateOf<String?>(null) }
    val camera=remember { YoCamera(context) }
    val permissions=rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { result ->
        cameraOn=result[Manifest.permission.CAMERA]==true
        if(!cameraOn) model.error(uiResources.getString(R.string.ui_allow_camera_access_in_android_settings_to_send_photos))
    }
    val picker=rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(10)) { picked=it.takeIf { it.isNotEmpty() }; hintEpoch++ }
    // The value this effect ran with: read in onDispose, the state is already the new one, and opening the camera
    // closed it again at once (the binding was dropped: a black preview, "Couldn't record video", until a flip).
    DisposableEffect(cameraOn) { val wasOn = cameraOn; onDispose { if(wasOn) camera.close() } }
    LaunchedEffect(cameraOn) { if (cameraOn) hintNeeded = true }
    LaunchedEffect(cameraOn,hintEpoch,picked) { hint=false; if(cameraOn && hintNeeded) { delay(3000); hint=true } }
    LaunchedEffect(cameraOn) { repo.camera(cameraOn) }
    // A Yo landing: a heavy tick, as iOS's arrival haptic (several in one frame play once).
    LaunchedEffect(repo) { repo.arrivals.collect { view.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS); withFrameNanos { } } }
    // The viewer opening: a selection tick.
    LaunchedEffect(viewing != null) { if (viewing != null) view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK) }
    val stacks by repo.media.undo.collectAsStateWithLifecycle()
    val thumbnails by repo.media.thumbnails.collectAsStateWithLifecycle()
    val friendsByKey = s.friends.associateBy { it.key }
    val friends = (s.listRows.keys.mapNotNull(friendsByKey::get) +
        s.friends.filter { it.key !in s.listRows && it.key != s.deletion?.id }).filterNot { it.blocked }
    val mediaByTarget = s.media.groupBy { it.target }
    // Two core calls a friend: asked when the friends change, not on each composition of the list.
    val realFriends = remember(friends) { friends.any {
        uniffi.yo.productIsRealFriend(uniffi.yo.usernameSkeleton(it.name), it.group != null, it.request, it.blocked, it.gone)
    } }
    val bars = friends.associate { friend ->
        friend.key to BarContent(friend, s.listRows[friend.key], stacks[friend.target],
            mediaByTarget[friend.target].orEmpty(), recording == friend.key, thumbnails[friend.target].orEmpty())
    }
    var fingersDown by remember { mutableStateOf(emptySet<String>()) }
    val visible = friends
    val heights = visible.associate { friend -> friend.key to bars.getValue(friend.key).lines }
    val tapLock = remember { BarTapLock() }
    val listState=androidx.compose.foundation.lazy.rememberLazyListState()
    SideEffect {
        val atTop = listState.firstVisibleItemIndex == 0 && listState.firstVisibleItemScrollOffset == 0
        if (tapLock.update(heights) && atTop) listState.requestScrollToItem(0)
    }
    fun touch(friend: Friend): Boolean = !tapLock.locked(friend.key) && recording == null
    fun press(friend: Friend, down: Boolean) {
        if (down && !tapLock.locked(friend.key)) {
            fingersDown = fingersDown + friend.key
            repo.press(friend.key, true)
        } else if (!down && friend.key in fingersDown) {
            fingersDown = fingersDown - friend.key
            repo.press(friend.key, false)
        }
    }
    // The name field opened: once the keyboard is up, the whole add block (the field, Group and Invite) shows above it,
    // as iOS scrolls to the block's bottom.
    LaunchedEffect(add, keyboardVisible) {
        if (add && keyboardVisible) listState.layoutInfo.totalItemsCount.takeIf { it > 0 }?.let { listState.animateScrollToItem(it - 1) }
    }
    PredictiveBackHandler(add && !addingFriend && !cameraOn) { events ->
        events.collect { }
        add = false
        name = ""
        addError = null
    }
    PredictiveBackHandler(recording!=null) { events -> events.collect { } }
    // Back with media picked: the live camera again; with nothing picked: the list.
    PredictiveBackHandler(recording==null && (cameraOn || picked!=null)) { events -> events.collect { }; if(picked!=null) picked=null else cameraOn=false }
    fun tap(friend: Friend) {
        Lat.mark("tap")
        if(importingShares) return
        if (shares.isNotEmpty() && (friend.request || friend.blocked || friend.gone)) return
        if(!touch(friend)) return
        if (friend.request) {
            view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
            repo.command {
                val session = sessionId
                answer(friend, true)
                if (sessionId == session) send(friend.copy(request = false, undo = false))
            }
            return
        }
        view.performHapticFeedback(if(android.os.Build.VERSION.SDK_INT>=30) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.VIRTUAL_KEY)
        hintEpoch++
        if (cameraOn) hintNeeded = false
        when {
            shares.isNotEmpty() -> model.stage(friend,shares)
            picked!=null -> { model.stage(friend,picked!!); picked=null }
            cameraOn -> {
                val session=repo.sessionId
                scope.launch {
                    try { val uri=camera.photo(); model.stage(friend,listOf(uri),true,session) }
                    catch(e: Exception) { model.error(e.message ?: uiResources.getString(R.string.ui_couldn_t_take_photo)) }
                }
            }
            // "Not Sent · Tap to Retry": what waits for this bar goes again, no new Yo.
            repo.state.value.listRows[friend.key]?.unsent == true -> repo.retryBar(friend)
            else -> repo.command { send(friend) }
        }
    }
    val currentTap by rememberUpdatedState<(Friend)->Unit>(::tap)
    // The bars' callbacks reach press/touch through these, so they stay the same lambdas from one composition to the
    // next (press and touch read this composition's heights) and an unchanged bar skips its recomposition.
    val currentPress by rememberUpdatedState<(Friend, Boolean)->Unit>(::press)
    val currentTouch by rememberUpdatedState<(Friend)->Boolean>(::touch)
    val listObscured = profile || group || menu != null || viewing != null
    LaunchedEffect(listState, listObscured) {
        if (listObscured) {
            repo.viewport(emptyList())
            return@LaunchedEffect
        }
        snapshotFlow { listState.layoutInfo.visibleItemsInfo.filter { item ->
            val info=listState.layoutInfo
            val visible=minOf(item.offset+item.size,info.viewportEndOffset)-maxOf(item.offset,info.viewportStartOffset)
            visible>=item.size*.6f
        }.map { it.key.toString() } }.collect { keys ->
            // The core's list call runs off the main thread: it changes as rows scroll in and out of view.
            withContext(Dispatchers.Default) { repo.viewport(keys) }
        }
    }
    fun toggleCamera() {
        if (recording != null) return
        if (cameraOn) {
            cameraOn = false
            picked = null
            camera.close()
        } else if (listObscured) {
            return  // never under a sheet (Settings, a group, a menu, the viewer)
        } else if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            cameraOn = true
        } else {
            permissions.launch(arrayOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO))
        }
    }
    val edgeCamera by rememberUpdatedState(::toggleCamera)
    val cameraAsked by model.openCamera.collectAsStateWithLifecycle()
    LaunchedEffect(cameraAsked, listObscured) { if (cameraAsked && !listObscured) { model.openCamera.value = false; if (!cameraOn) toggleCamera() } }
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
                        view.performHapticFeedback(if (Build.VERSION.SDK_INT >= 30) HapticFeedbackConstants.GESTURE_END else HapticFeedbackConstants.VIRTUAL_KEY)
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
    fun hold(friend: Friend, held: Boolean) {
        if(held && touch(friend)) {
            hintEpoch++
            hintNeeded = false
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
                recording=null
                repo.recording(null)
            }
        } else if(!held && recording==friend.key) {
            view.performHapticFeedback(if (Build.VERSION.SDK_INT >= 30) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.VIRTUAL_KEY)
            camera.stopVideo()
        }
        if(!held) camera.recordZoomEnd()
    }
    // One object for every bar's callbacks, the same from one composition to the next: a bar whose content did not
    // change skips its recomposition when the list recomposes (each Yo, each viewport change), as do its swipe box.
    val rowActions = remember { RowActions() }
    rowActions.tap = { currentTap(it) }
    rowActions.press = { friend, down -> currentPress(friend, down) }
    // No menu in camera mode (iOS): a long press there is a hold.
    rowActions.menu = { friend -> if(!cameraOn && !tapLock.locked(friend.key)) menu=friend }
    rowActions.undo = { friend -> if(currentTouch(friend)) repo.command { cancelMedia(friend) } }
    rowActions.media = { friend -> heldFinger.point=null;if(currentTouch(friend)) viewing=MediaQueue.openable(repo.state.value.media,friend.target).takeIf { it.isNotEmpty() } }
    rowActions.mediaTouch = { when(it) {
        is MediaTouch.Moved -> { heldFinger.barBottom=it.barBottom;heldFinger.point=it.point }
        MediaTouch.Tapped -> heldFinger.point=null
        MediaTouch.Released -> { heldFinger.point=null;viewing=null }
    } }
    // "No" declines at once, as on iOS (a tap's haptic; blocking is the long press's).
    rowActions.decline = { friend -> if(currentTouch(friend)) { view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY); repo.command { answer(friend,false) } } }
    rowActions.hold = ::hold
    rowActions.holdDrag = { friend, y, top, bottom, height ->
        if(recording==friend.key && camera.recordZoom(y,top,bottom,height))
            view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
    }
    rowActions.remove = { friend -> repo.requestRemove(friend) }
    Box(Modifier.fillMaxSize()) {
        if (cameraOn) key(cameraVersion) {
            AndroidView(
                factory = { context ->
                    PreviewView(context).apply {
                        implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                        scaleType = PreviewView.ScaleType.FILL_CENTER
                        camera.bind(owner, this) { message ->
                            cameraOn = false
                            model.error(message)
                        }
                    }
                },
                modifier = Modifier.fillMaxSize(),
            )
        }
        picked?.let { items ->
            // The picked items behind the bars: one fills the screen, more share it in a grid.
            if (items.size == 1) PhotoUri(items[0], Modifier.fillMaxSize())
            else Column(Modifier.fillMaxSize()) {
                items.chunked(2).forEach { pair -> Row(Modifier.weight(1f)) { pair.forEach { PhotoUri(it, Modifier.weight(1f).fillMaxHeight()) } } }
            }
        }
        // The picture shows through Amethyst where no bar is, as it shows through each bar's colour (iOS FriendList).
        if (cameraOn) {
            val listTop = WindowInsets.safeDrawing.getTop(LocalDensity.current).toFloat()
            Canvas(Modifier.fillMaxSize()) {
                val tint = Amethyst.copy(alpha = 0.45f)
                val info = listState.layoutInfo
                val last = info.visibleItemsInfo.lastOrNull()
                val rowsEnd = when {
                    last == null -> 0
                    last.index == info.totalItemsCount - 1 -> last.offset + last.size
                    else -> info.viewportEndOffset
                }
                val rowsBottom = listTop + rowsEnd
                drawRect(tint, size = androidx.compose.ui.geometry.Size(size.width, listTop))
                if (rowsBottom < size.height) drawRect(tint, androidx.compose.ui.geometry.Offset(0f, rowsBottom),
                    androidx.compose.ui.geometry.Size(size.width, size.height - rowsBottom))
            }
        }
        Box(Modifier.fillMaxSize().safeDrawingPadding().then(edgeGesture)) {
        Column(Modifier.fillMaxSize()) {
        // "Offline" or a failed send (tap: again) above the list, never over its first bar (iOS's top inset).
        androidx.compose.animation.AnimatedVisibility(s.banner != null && s.deletion == null) {
            ConnectionBanner(s.banner.orEmpty(), onRetry = repo::bannerTapped)
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f),state=listState,userScrollEnabled=recording==null,contentPadding=PaddingValues(bottom=if (keyboardVisible) 0.dp else 84.dp)) {
            if (showNotificationBar && !cameraOn) item(key = "notification-settings") {
                NotificationSettingsRow { openNotificationSettings(context) }
            }
            items(visible.size,key={visible[it].key}) { index ->
                val friend = visible[index]
                FriendRow(repo, friend, bars.getValue(friend.key), rowShade(visible,index), cameraOn, recording, s.leftHanded,
                    holdable = cameraOn && picked==null && shares.isEmpty() && !friend.request, actions = rowActions)
            }
            if(!cameraOn) {
                item {
                    if (add) {
                        val placeholder = lookingUp ?: addError ?: stringResource(R.string.ui_name)
                        // "No Such User", "That’s You", "No Connection": a moment in the field, then the name prompt again.
                        LaunchedEffect(addError) { if (addError != null) { delay(1500); addError = null } }
                        val style = TextStyle(fontFamily = YoFont, fontSize = 36.sp, color = Color.White, textAlign = TextAlign.Center)
                        val (nameValue, onName) = nameField(name) { name = it; addError = null }
                        BasicTextField(
                            value = nameValue,
                            onValueChange = onName,
                            singleLine = true,
                            enabled = true,
                            textStyle = style,
                            cursorBrush = SolidColor(Color.White),
                            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = { submitFriend() }),
                            modifier = Modifier.fillMaxWidth().height(BarHeight).focusRequester(friendFocus)
                                .semantics { contentDescription = placeholder },
                            decorationBox = { field ->
                                Box(
                                    Modifier.fillMaxSize().background(RowColors[Repository.nextColor(s.friends)])
                                        .padding(horizontal = 20.dp),
                                    contentAlignment = Alignment.Center,
                                ) {
                                    if (nameValue.text.isEmpty()) Text(
                                        placeholder, style = style.copy(color = Color.White.copy(alpha = 0.45f)),
                                        maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                                    )
                                    field()
                                }
                            },
                        )
                        // Open, and again after "No Such User": the keyboard stays for the next name, as on iOS.
                        LaunchedEffect(addError) { friendFocus.requestFocus() }
                    } else {
                        val addFriendLabel = stringResource(R.string.ui_add_friend)
                        FlatRow("+", Repository.nextColor(s.friends), Modifier.semantics {
                            contentDescription = addFriendLabel
                        }, onClick = { add = true })
                    }
                }
                val hasContacts = realFriends
                if (!hasContacts || add) {
                    item {
                        Row {
                            ListActionRow(
                                stringResource(R.string.group_row), Icons.Default.GroupAdd, Color(0xFF3498DB),
                                Modifier.weight(1f),
                            ) {
                                focusManager.clearFocus()
                                add = false
                                name = ""
                                addError = null
                                group = true
                            }
                            ListActionRow(
                                stringResource(R.string.ui_invite), Icons.Default.Share, Amethyst,
                                Modifier.weight(1f),
                            ) { shareInvite(context, s.username.orEmpty()) }
                        }
                    }
                }
            }
        }
        }
        if (!keyboardVisible) Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(horizontal = 16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            val glassTint = Color.White.copy(alpha = 0.2f)
            val prompt = when {
                importingShares -> stringResource(R.string.preparing_shared_media)
                sharing -> stringResource(R.string.share_selected_media)
                hint && cameraOn && picked != null -> stringResource(R.string.ui_tap_a_name_to_send_selected_media)
                hint && cameraOn -> stringResource(R.string.ui_tap_a_name_hold_for_video)
                else -> null
            }
            if (prompt != null) {
                Surface(shape = CircleShape, color = glassTint, contentColor = Color.White) {
                    Box(Modifier.heightIn(min = 44.dp).padding(horizontal = 18.dp), contentAlignment = Alignment.Center) {
                        Text(prompt, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
                    }
                }
            }
            Box(Modifier.fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
                val controls: @Composable () -> Unit = {
                    if (cameraOn) {
                        FloatingActionButton(
                            onClick = {
                                if (recording == null) picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo))
                            },
                            shape = CircleShape,
                            containerColor = glassTint,
                            contentColor = Color.White,
                            elevation = FloatingActionButtonDefaults.elevation(0.dp, 0.dp, 0.dp, 0.dp),
                        ) {
                            Icon(Icons.Default.PhotoLibrary, stringResource(R.string.ui_photo_library), Modifier.size(26.dp))
                        }
                    } else {
                        FloatingActionButton(
                            onClick = { profile = true },
                            shape = CircleShape,
                            containerColor = glassTint,
                            contentColor = Color.White,
                            elevation = FloatingActionButtonDefaults.elevation(0.dp, 0.dp, 0.dp, 0.dp),
                            modifier = Modifier.clearAndSetSemantics {
                                this.contentDescription = uiResources.getString(R.string.ui_settings)
                                role = androidx.compose.ui.semantics.Role.Button
                                onClick { profile = true; true }
                            },
                        ) {
                            // The red dot: the account is not backed up (Settings › Account Recovery says why).
                            BadgedBox(badge = { if (s.backupMissing) Badge() }) {
                                Row(horizontalArrangement = Arrangement.spacedBy((-5).dp)) {
                                    Text("Y", Modifier.alignByBaseline(), style = TextStyle(fontFamily = YoFont, fontSize = 29.sp))
                                    Text("o", Modifier.alignByBaseline(), style = TextStyle(fontFamily = YoFont, fontSize = 23.sp))
                                }
                            }
                        }
                    }
                }
                val cameraButton: @Composable () -> Unit = {
                    val clear = sharing || (cameraOn && picked != null)
                    FloatingActionButton(
                        onClick = {
                            when {
                                sharing -> model.cancelShares()
                                clear -> picked = null
                                else -> toggleCamera()
                            }
                        },
                        shape = CircleShape,
                        containerColor = glassTint,
                        contentColor = Color.White,
                        elevation = FloatingActionButtonDefaults.elevation(0.dp, 0.dp, 0.dp, 0.dp),
                    ) {
                        Icon(
                            if (cameraOn || clear) Icons.Default.Close else Icons.Default.CameraAlt,
                            stringResource(
                                if (clear) R.string.clear_selection
                                else if (cameraOn) R.string.ui_close_camera
                                else R.string.ui_camera
                            ),
                            modifier = Modifier.size(26.dp),
                        )
                    }
                }
                val mirrored = s.leftHanded && !sharing
                val gallerySide = if (mirrored) Alignment.CenterEnd else Alignment.CenterStart
                val cameraSide = if (mirrored) Alignment.CenterStart else Alignment.CenterEnd
                Box(Modifier.align(gallerySide)) { controls() }
                if (cameraOn && picked == null && camera.zoomChoices.size > 1) {
                    IconButton(
                        onClick = { view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK); camera.zoom() }, enabled = recording == null,
                        modifier = Modifier.size(44.dp),
                        colors = IconButtonDefaults.iconButtonColors(containerColor = glassTint, contentColor = Color.White),
                    ) { Text(uniffi.yo.productCameraZoomLabel(camera.zoom.toDouble()) + "×", fontSize = 13.sp, fontWeight = FontWeight.Bold, maxLines = 1) }
                }
                Row(Modifier.align(cameraSide), horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    if (mirrored) cameraButton()
                    // Media picked: nothing to flip, only the library and the clear button stay.
                    if (cameraOn && picked == null) IconButton(
                        onClick = { view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY); camera.flip(); cameraVersion++ }, enabled = recording == null,
                        modifier = Modifier.size(56.dp),
                        colors = IconButtonDefaults.iconButtonColors(containerColor = glassTint, contentColor = Color.White),
                    ) {
                        Icon(Icons.Default.Cameraswitch, stringResource(R.string.ui_switch_camera), Modifier.size(26.dp))
                    }
                    if (!mirrored) cameraButton()
                }

            }
        }
    }
    }
    if (profile) ProfileSheet(model, onClose = { profile = false }, onAddFriends = {
        profile = false
        add = true
        scope.launch { listState.animateScrollToItem(visible.size + if (showNotificationBar) 1 else 0) }
    })
    if(group) GroupSheet(model,s,onClose={group=false})
    menu?.let { friend -> FriendMenu(model,friend,onClose={menu=null}) }
    viewing?.let { queue -> MediaViewer(repo,queue,heldFinger,onClose={viewing=null}) }
}

fun shareInvite(context: Context,name: String) {
    context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT,context.getString(R.string.ui_yo_add_me_on_yo,"https://justyo.co/${Uri.encode(name)}")),context.getString(R.string.ui_invite_friends)))
}
@Composable fun PhotoUri(uri: Uri,modifier: Modifier=Modifier) {
    val context=LocalContext.current
    var bitmap by remember(uri) { mutableStateOf<android.graphics.Bitmap?>(null) }
    LaunchedEffect(uri) { bitmap=withContext(Dispatchers.IO) { runCatching {
        if (context.contentResolver.getType(uri)?.startsWith("video/") == true)
            return@runCatching android.media.MediaMetadataRetriever().run {
                try { setDataSource(context, uri); getScaledFrameAtTime(0, android.media.MediaMetadataRetriever.OPTION_CLOSEST_SYNC, 1024, 1024) }
                finally { release() }
            }
        android.graphics.ImageDecoder.decodeBitmap(android.graphics.ImageDecoder.createSource(context.contentResolver,uri)) { d,info,_ ->
        val ratio=minOf(1.0,2048.0/maxOf(info.size.width,info.size.height));d.setTargetSize((info.size.width*ratio).toInt().coerceAtLeast(1),(info.size.height*ratio).toInt().coerceAtLeast(1));d.allocator=android.graphics.ImageDecoder.ALLOCATOR_SOFTWARE
    } }.getOrNull() } }
    bitmap?.let { Image(it.asImageBitmap(),contentDescription=null,modifier=modifier,contentScale=ContentScale.Crop) }
}

@Composable
private fun OnboardingButton(label: String, enabled: Boolean, onClick: () -> Unit) {
    Button(
        onClick = onClick, enabled = enabled, shape = CircleShape,
        modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp),
        colors = ButtonDefaults.buttonColors(
            containerColor = Color.White, contentColor = Color.Black,
            disabledContainerColor = Color.White.copy(alpha = 0.4f),
            disabledContentColor = Color.Black.copy(alpha = 0.6f),
        ),
    ) {
        Text(label, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun OnboardingNote(message: String?) {
    if (message != null) Text(message, Modifier.padding(top = 8.dp), color = Color.White,
        fontSize = 13.sp, lineHeight = 18.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
}

/** A name field's text. A Japanese keyboard hands the field kana and full-width letters while a word is written ("i"
 *  arrives as "い"): cleaning every change emptied the field. Left alone while composing; once committed, read as Latin
 *  and cleaned (as iOS's Username.clean). */
@Composable
private fun nameField(name: String, onChange: (String) -> Unit): Pair<TextFieldValue, (TextFieldValue) -> Unit> {
    var field by remember { mutableStateOf(TextFieldValue(name, TextRange(name.length))) }
    val shown = if (field.composition == null && field.text != name) TextFieldValue(name, TextRange(name.length)) else field
    return shown to { value ->
        if (value.composition != null) field = value
        else {
            val clean = usernameCanonical(latinName(value.text))
            field = TextFieldValue(clean, TextRange(clean.length))
            if (clean != name) onChange(clean)
        }
    }
}

/** Kana and full-width letters as the Latin a romaji keyboard typed; anything else unchanged. */
internal fun latinName(typed: String): String {
    if (typed.none { it in '\u3040'..'\u30FF' || it in '\uFF01'..'\uFF9F' }) return typed
    val narrow = java.text.Normalizer.normalize(typed, java.text.Normalizer.Form.NFKC)
    return if (Build.VERSION.SDK_INT >= 29) android.icu.text.Transliterator.getInstance("Hiragana-Latin; Katakana-Latin")
        .transliterate(narrow) else narrow
}

@Composable
private fun NameEntry(value: TextFieldValue, onChange: (TextFieldValue) -> Unit, available: Boolean?, modifier: Modifier, onSubmit: () -> Unit) {
    val name = value.text
    val placeholder = copy(ProductText.USERNAME)
    val measure = rememberTextMeasurer()
    val density = LocalDensity.current
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val style = TextStyle(fontFamily = YoFont, fontSize = 28.sp, color = Color.White, textAlign = TextAlign.Center)
        val naturalWidth = measure.measure(name, style, maxLines = 1, softWrap = false).size.width.coerceAtLeast(1)
        val room = with(density) { (maxWidth - 88.dp).coerceAtLeast(0.dp).toPx() }
        val size = (28 * room / naturalWidth).coerceIn(8f, 28f).sp
        BasicTextField(
            value = value, onValueChange = onChange, singleLine = true,
            modifier = modifier.fillMaxWidth().semantics { contentDescription = placeholder },
            textStyle = style.copy(fontSize = size), cursorBrush = SolidColor(Color.White),
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { onSubmit() }),
            decorationBox = { field ->
                Box(
                    Modifier.fillMaxWidth().background(Color.White.copy(alpha = 0.16f), CircleShape)
                        .padding(vertical = 14.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Box(Modifier.fillMaxWidth().padding(horizontal = 44.dp).heightIn(min = 34.dp),
                        contentAlignment = Alignment.Center) {
                        if (name.isEmpty()) Text(placeholder, Modifier.fillMaxWidth(),
                            style = style.copy(color = Color.White.copy(alpha = 0.45f)), maxLines = 1)
                        field()
                    }
                    if (available != null) Box(
                        Modifier.align(Alignment.CenterEnd).padding(end = 14.dp).size(24.dp)
                            .background(if (available) Color(0xFF34C759) else Color(0xFFFF3B30), CircleShape),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(if (available) Icons.Default.Check else Icons.Default.Close,
                            if (available) stringResource(R.string.ui_available) else copy(ProductText.NAME_UNAVAILABLE),
                            Modifier.size(17.dp), tint = Color.White)
                    }
                }
            },
        )
    }
}

/** A bar's callbacks, kept in one stable object (its fields set by the list on each composition). */
internal class RowActions {
    var tap: (Friend) -> Unit = {}
    var press: (Friend, Boolean) -> Unit = { _, _ -> }
    var menu: (Friend) -> Unit = {}
    var undo: (Friend) -> Unit = {}
    var media: (Friend) -> Unit = {}
    var mediaTouch: (MediaTouch) -> Unit = {}
    var decline: (Friend) -> Unit = {}
    var hold: (Friend, Boolean) -> Unit = { _, _ -> }
    var holdDrag: (Friend, Float, Float, Float, Float) -> Unit = { _, _, _, _, _ -> }
    var remove: (Friend) -> Unit = {}
}

/** One row of the list: the swipe to remove around the bar. Skipped when nothing it shows changed. */
@Composable internal fun FriendRow(repo: Repository, friend: Friend, bar: BarContent, shade: Color, cameraOn: Boolean, recording: String?,
    leftHanded: Boolean, holdable: Boolean, actions: RowActions) {
    val dismiss = rememberSwipeToDismissBoxState(confirmValueChange={ value ->
        if(value==SwipeToDismissBoxValue.EndToStart) { actions.remove(friend);false } else true
    })
    val res = androidx.compose.ui.platform.LocalResources.current
    // As iOS's bar actions: filming without holding, and the menu's and swipe's actions for screen readers.
    val a11y = Modifier.semantics {
        customActions = buildList {
            if (holdable) add(androidx.compose.ui.semantics.CustomAccessibilityAction(
                res.getString(if (recording == friend.key) R.string.stop_and_send_video else R.string.record_video)) {
                actions.hold(friend, recording != friend.key); true })
            if (!cameraOn && !friend.request) {
                add(androidx.compose.ui.semantics.CustomAccessibilityAction(res.getString(R.string.contact_actions)) { actions.menu(friend); true })
                add(androidx.compose.ui.semantics.CustomAccessibilityAction(res.getString(R.string.ui_remove)) { actions.remove(friend); true })
            }
        }
    }
    SwipeToDismissBox(modifier=a11y,state=dismiss,enableDismissFromStartToEnd=false,enableDismissFromEndToStart=!cameraOn && recording==null && !friend.request,
        backgroundContent = {
            // Drawn only while the bar is swiped: otherwise every frame painted it under each bar.
            val swiping by remember(dismiss) { derivedStateOf { dismiss.dismissDirection != SwipeToDismissBoxValue.Settled } }
            if (!cameraOn && swiping) {
                Box(
                    Modifier.fillMaxSize().background(MaterialTheme.colorScheme.error).padding(horizontal = 24.dp),
                    contentAlignment = Alignment.CenterEnd,
                ) {
                    Icon(Icons.Default.Delete, stringResource(R.string.ui_remove))
                }
            }
        },
    ) {
        FriendBar(repo, bar, shade, cameraOn, leftHanded,
            onPress={actions.press(friend,it)},onTap={actions.tap(friend)},onMenu={actions.menu(friend)},
            onUndo={actions.undo(friend)},onMedia={actions.media(friend)},onMediaTouch={actions.mediaTouch(it)},
            onDecline={actions.decline(friend)},
            onHold=if(holdable) { held -> actions.hold(friend, held) } else null,
            onHoldDrag={ y, top, bottom, height -> actions.holdDrag(friend, y, top, bottom, height) })
    }
}

/** iOS's top line: Amethyst, a red dot, Montserrat; "Offline" with its dots going, a failure tappable to try again. */
@Composable private fun ConnectionBanner(text: String, onRetry: () -> Unit) {
    val offline = text == "Offline"
    val dots by produceState(0, offline) { while (offline) { delay(450); value = (value + 1) % 4 } }
    Row(
        Modifier.fillMaxWidth().height(36.dp).background(Amethyst)
            .then(if (offline) Modifier else Modifier.clickable(onClick = onRetry)),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(7.dp).background(Color.Red, CircleShape))
        val style = TextStyle(fontFamily = YoFont, fontSize = 16.sp, color = Color.White)
        Text(text, maxLines = 1, style = style)
        // The dots in a box of their own width: the word stays where it is while they come and go.
        if (offline) Box(Modifier.padding(start = 0.dp).width(18.dp).offset(x = (-6).dp)) { Text(".".repeat(dots), maxLines = 1, style = style) }
    }
}
