package app.justyo.android

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.util.Base64
import androidx.glance.appwidget.updateAll
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import uniffi.yo.*

/** One client in one application process; share intents use this same repository, never a second writer. */
class Repository(private val context: Context, ownerScope: CoroutineScope) {
    private val mutable = MutableStateFlow(AppState())
    val state = mutable.asStateFlow()
    private val scope = CoroutineScope(ownerScope.coroutineContext + CoroutineExceptionHandler { _, error ->
        if (error is Exception && error !is CancellationException) {
            mutable.update { it.copy(error = friendlyError(context,error)) }
        } else if (error !is CancellationException) throw error
    })
    private val vault = Vault(context)
    private val synced = SyncedAccount(context)
    @Volatile private var passkeyBackup = false
    private val file by lazy {
        CoreState(context, object : StateDiagnostics {
            override fun failure(kind: String) = crashes.countStateFailure(kind)
        })
    }
    val transport = HubTransport { mutable.update { it.copy(update = true) } }
    @Volatile private var client: YoClient? = null
    @Volatile var sessionId: Long = 0
        private set
    private val changes = Mutex()
    internal val pendingShares=PendingShares(context,vault,changes) { sessionId }
    private val accountTurn = Mutex()
    private val sending = mutableMapOf<String, Mutex>()
    private var list = YoList()
    private var listJob: Job? = null
    private val listLock = Any()
    private var orderSave: Job? = null
    private var displayedOrder=emptyList<String>()
    private fun now() = android.os.SystemClock.elapsedRealtime().toULong()
    /**
     * A row's clock runs while the row is seen, and it shows only in a fade (a received Yo, a group's senders). For a
     * row without one, scrolling it in or out of view changes nothing on screen: it keeps its old value, so the state
     * stays equal and neither the list nor the widget recomposes on every scroll.
     */
    private fun steadyRows(old: Map<String, ListRow>, rows: List<ListRow>) = rows.associate { row ->
        val previous = old[row.id]
        row.id to (previous?.takeIf {
            row.fadeElapsed == null && row.senders.isEmpty() && it.clockRunning != row.clockRunning &&
                it.copy(clockRunning = row.clockRunning) == row
        } ?: row)
    }
    private fun present(work: YoList.(ULong) -> ListSnapshot): Unit = synchronized(listLock) {
        val snapshot = list.work(now())
        val order=snapshot.rows.map { it.id }
        val orderChanged=order!=displayedOrder && snapshot.deletion==null && snapshot.committedDeletes.isEmpty()
        if(orderChanged) displayedOrder=order
        mutable.update { state ->
            // Merge by key with the latest source, retaining contacts added concurrently with this tick.
            val byKey=state.friends.associateBy { it.key }
            val ordered=if(orderChanged) state.friends.filter { it.key !in order }+order.mapNotNull(byKey::get) else state.friends
            state.copy(friends=ordered,listRows = steadyRows(state.listRows, snapshot.rows), lockedRow = snapshot.lockedRow,deletion=snapshot.deletion)
        }
        if(orderChanged) {
            orderSave?.cancel()
            val session=sessionId
            orderSave=scope.launch(Dispatchers.IO) {
                delay(150)
                changes.withLock { if(session==sessionId && !state.value.loading) persist() }
            }
        }
        snapshot.committedDeletes.forEach { id ->
            state.value.friends.firstOrNull { it.key==id }?.let { friend -> scope.launch(Dispatchers.IO) { remove(friend) } }
        }
        listJob?.cancel()
        listJob = snapshot.nextTick?.let { at -> scope.launch(Dispatchers.IO) {
            delay((at.toLong() - now().toLong()).coerceAtLeast(16))
            present { tick(it) }
        } }
    }
    private fun listFriends() = present { time ->
        friends(time, state.value.friends.map { ListFriend(it.key, it.name, it.request, it.group != null) })
        state.value.friends.forEach { f -> f.user?.let { senderNamed(time, it, f.name) } }
        tick(time)
    }
    fun presentationUndo(friend: Friend, undo: UndoStack?) = present { time ->
        if(undo == null) tick(time) else sendStarted(time, friend.key, undo.kind,
            (undo.until-android.os.SystemClock.elapsedRealtime()).coerceAtLeast(0).toULong())
    }
    suspend fun cancelMedia(friend: Friend) { media.cancel(friend.target);present { sendUndone(it,friend.key) } }
    fun requestRemove(friend: Friend) = present { delete(it,friend.key) }
    fun undoRemove() = present { undoDelete(it) }
    fun viewport(keys: List<String>) = present { rowsVisible(it, keys) }
    fun camera(on: Boolean) = present { camera(it, on) }
    fun recording(key: String?) = present { recording(it, key) }
    fun press(key: String, down: Boolean) = present { press(it, key, down) }
    private val seen = LinkedHashSet<String>()
    private val removedUsers = mutableSetOf<String>()
    @Volatile private var foreground = false
    private val syncing = Mutex()
    private var listening: Job? = null
    /** A network came (back): the listener stops waiting out its backoff (up to 30 s) and the queues retry at once. */
    private val online = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
    private var retrying: Job? = null
    private var resuming: Job? = null
    val media = MediaQueue(context,vault, scope, { client }, { sessionId }, { source, events -> absorb(events, source) }, { refreshMedia() }).also { queue ->
        queue.sendYo = { friend -> client?.let { sendNow(friend, it) } }
        queue.sent = { target, last ->
            state.value.friends.firstOrNull { it.target == target }?.let {
                if (last) showSent(it)
                recordStat(it, true)
            }
        }
    }
    init {
        runCatching {
            context.getSystemService(android.net.ConnectivityManager::class.java)
                .registerDefaultNetworkCallback(object : android.net.ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: android.net.Network) {
                        online.trySend(Unit)
                        if (foreground && client != null) media.wake()
                    }
                })
        }
    }
    private val soundPlayers = mutableSetOf<MediaPlayer>()
    fun start() = scope.launch(Dispatchers.IO) {
        scope.launch(Dispatchers.IO) {
            state.map { it.username to it.friends }.distinctUntilChanged().collect { (_, friends) ->
                // Shortcut publishing is advisory; launcher rejection must not break the account.
                runCatching { updateSharingShortcuts(context,friends) }
                runCatching { YoWidget().updateAll(context) }
            }
        }
        scope.launch {
            var previous=emptyMap<String,UndoStack>()
            media.undo.collect { stacks ->
                state.value.friends.forEach { f -> stacks[f.target]?.let { stack ->
                    if(stack != previous[f.target]) repeat((stack.count-(previous[f.target]?.count ?: 0)).coerceAtLeast(0)) {
                        presentationUndo(f,stack)
                    }
                } }
                previous=stacks
            }
        }
        try {
            scope.launch(Dispatchers.IO) { runCatching { crashes.flush() } }
            // A process may have ended while viewing or normalizing media. Plain temporary files never survive a launch.
            clearTemporaryMedia()
            // The one migration: an older install's sign-in is kept first (`migrate`), then its files go.
            try { migrate() } catch (e: YoException.Network) {
                mutable.update { it.copy(loading = false, updating = true) }
                if (foreground) keepResuming()
                return@launch
            }
            val cached = vault.read("ui")?.let { JSONObject(String(it)) }
            cached?.optJSONArray("removed")?.let { array -> for (i in 0 until array.length()) removedUsers.add(array.getString(i)) }
            if (cached != null) mutable.update { it.copy(
                username = cached.stringOrNull("username"),
                friends = cached.optJSONArray("friends")?.objects()?.map(Friend::from).orEmpty(),
                byFriend=cached.longMap("byFriend"),lastSent=cached.longMap("lastSent"),lastReceived=cached.longMap("lastReceived"),
                secured = cached.optBoolean("secured"), leftHanded = cached.optBoolean("leftHanded"),
                saveSent = cached.optBoolean("saveSent"), sent = cached.optLong("sent"), received = cached.optLong("received"),
                pendingInvite = cached.stringOrNull("invite"), appIcon = cached.stringOrNull("appIcon"),
                googleBackup = cached.optBoolean("googleBackup", true) && backupWanted(),
            ) }
            // An opt-out of before only in `ui`: kept where it outlives a sign-out.
            if (cached?.optBoolean("googleBackup", true) == false) vault.write("google-backup", byteArrayOf(0))
            if (cached == null) mutable.update { it.copy(googleBackup = backupWanted()) }
            val key = vault.read("state-key")
            val restored = if (file.exists()) {
                key ?: error(context.getString(R.string.ui_state_key_missing_use_your_recovery_key))
                val db = file.database(key)
                // A kept sign-in of `migrate` saved before the hub answered is finished by `finishMigration`.
                if (db.hasState() && (vault.read("migration") == null || db.registered())) YoClient.openDb(transport, db, key, false, null) else null
            } else null
            if (restored != null) {
                vault.delete("migration"); vault.delete("migration-storage")
                pin(restored)
                client = restored; sessionId++
                mutable.update { it.copy(username = restored.username(), recovery = vault.read("recovery")?.decodeToString()) }
                media.load()
                absorb(restored.savedEvents(), restored)
                refresh()
                refreshMedia()
                media.wake()
                deliverLater(restored)
                scope.launch(Dispatchers.IO) { syncItem() }
            } else if (state.value.username != null) error(context.getString(R.string.ui_account_state_missing_use_your_recovery_key))
            // The automatic sign-in (iOS: from the iCloud Keychain): an old install's kept sign-in, else the synced item.
            if (restored == null && !finishMigration()) {
                val unmarked = !settleSynced()
                val item = runCatching { synced.read() }.getOrNull()
                if (SyncedAccount.signsIn(item, vault.read("left"), unmarked)) runCatching { signIn(item!!) }
            }
            mutable.update { it.copy(loading = false) }
            if (foreground) {
                if (client != null) YoWork.registerIfConfigured(context)
                listen()
                if (state.value.updating) keepResuming()
            }
            consumeInvite()
        } catch (e: Exception) {
            mutable.update { it.copy(loading = false, broken = true, error = friendlyError(context,e)) }
        }
    }
    /**
     * An older install (state files of before the current schema): its sign-in (username, account key from the item
     * of then or its state, recovery key) is kept in the vault, then the old files and vault entries go. Throws
     * `Network` while the old state is not readable now; nothing is deleted then.
     */
    private fun migrate() {
        if (!file.hasOld()) return
        if (vault.read("migration") == null) {
            val username = vault.read("ui")?.let { JSONObject(String(it)).stringOrNull("username") }
            val identity = file.legacy(vault.read("state-key"))
            pendingSignIn(username, identity, vault.read("recovery")?.decodeToString())?.let { item ->
                identity?.storageKey?.let { vault.write("migration-storage", it) }
                vault.write("migration", syncedItemEncode(item))
            }
        }
        vault.clear(); file.deleteOld()
    }

    /**
     * Signs the kept sign-in of `migrate` in; it is removed only once the hub answered. Not reachable: kept, "Yo is
     * updating…", tried again in the foreground. True while there was one.
     */
    private suspend fun finishMigration(): Boolean {
        val kept = vault.read("migration")?.let(::syncedItemDecode) ?: return false
        try {
            mutable.update { it.copy(updating = true) }
            if (kept.accountKey.isEmpty()) restore(checkNotNull(kept.recoveryKey))
            else signIn(checkNotNull(syncedItemUpgraded(kept)), vault.read("migration-storage"))
        } catch (e: CancellationException) { throw e } catch (e: Exception) {
            // Not signed in: offline or the hub busy for now (`retryLater`, as iOS), kept for later; refused for good
            // (account gone, name another's), signed out as an old install without a key. Signed in and a later
            // step failed: done all the same.
            if (client == null && keepsPendingSignIn(e)) return true
            if (client == null) mutable.update { it.copy(error = friendlyError(context, e)) }
        }
        vault.delete("migration"); vault.delete("migration-storage")
        mutable.update { it.copy(updating = false) }
        return true
    }

    /**
     * "Yo is updating…" tried again by itself while the app is in the foreground, 2 s … 60 s apart, until it is done:
     * offline or a busy hub at the first start never leaves the spinner standing.
     */
    private fun keepResuming() {
        if (resuming?.isActive == true) return
        resuming = scope.launch(Dispatchers.IO) {
            var pause = 2_000L
            while (isActive && foreground && state.value.updating) {
                try { resumeUpdate() } catch (e: CancellationException) { throw e } catch (e: Exception) {
                    mutable.update { it.copy(error = friendlyError(context, e)) }
                }
                if (!state.value.updating) break
                delay(pause); pause = (pause * 2).coerceAtMost(60_000L)
            }
        }
    }

    /** The update tried again in the foreground: the old files, then the kept sign-in. */
    private suspend fun resumeUpdate() {
        try { migrate() } catch (e: YoException.Network) { return }
        if (!finishMigration()) mutable.update { it.copy(updating = false) }
        if (client != null) { listen(); YoWork.registerIfConfigured(context) }
    }

    /**
     * A sign-out or deletion that did not reach Block Store yet (`synced-pending`: "signedOut|delete <user id>"),
     * made now. True when none is left; until then no automatic sign-in.
     */
    private fun settleSynced(): Boolean {
        val (change, user) = vault.read("synced-pending")?.decodeToString()?.split(" ") ?: return true
        return runCatching {
            val item = synced.read()
            if (item != null && item.userId.hex().equals(user, ignoreCase = true)) {
                if (change == "delete") synced.delete() else synced.write(item.copy(signedOut = true), backupWanted())
            }
            vault.delete("synced-pending")
        }.isSuccess
    }

    private fun pin(c: YoClient) {
        c.setHubKeys(listOf("LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg").map(::decodeUrl))
        c.setTrustRoots(listOf("kKVskVCuVwESdvLeYWAEVAQmT9mdwvkQ2uhOlnxXO3Q", "Fj-t9EAOsTDPSHPT1Y39FdbcIvl2I7aEEdqlcRAQc0c").map(::decodeUrl))
        c.setZkParams(decodeUrl(DEV_GROUP_PARAMS))
    }
    private suspend fun persist() {
        val s = state.value
        vault.write("ui", JSONObject().put("username", s.username).put("friends", JSONArray(s.friends.map { it.json() }))
            .put("byFriend",JSONObject(s.byFriend)).put("lastSent",JSONObject(s.lastSent)).put("lastReceived",JSONObject(s.lastReceived))
            .put("appIcon", s.appIcon).put("secured", s.secured).put("leftHanded", s.leftHanded).put("saveSent", s.saveSent)
            .put("sent", s.sent).put("received", s.received).put("invite", s.pendingInvite).put("removed", JSONArray(removedUsers.toList()))
            .put("googleBackup", s.googleBackup).toString().toByteArray())
    }
    fun command(work: suspend Repository.() -> Unit) = scope.launch(Dispatchers.IO) {
        try { work() } catch (e: CancellationException) { throw e } catch (e: Exception) {
            if (e is YoException.Replaced) signOut(replaced = true) else mutable.update { it.copy(error = friendlyError(context,e)) }
        }
    }
    suspend fun register(typed: String) = accountTurn.withLock {
        check(client == null)
        mutable.update { it.copy(busy = true) }
        try {
            val name = usernameCanonical(typed)
            require(usernameValid(name)) { context.getString(R.string.valid_name) }
            val pendingName = "registration-" + usernameSkeleton(name).toByteArray().sha256Hex()
            val secret = vault.secret(pendingName)
            val c = registerCore(name, secret)
            pin(c); client = c; sessionId++; vault.delete("left")
            mutable.update { it.copy(username = c.username() ?: name, friends = emptyList(), broken = false) }
            persist(); vault.delete(pendingName)
            // YoGPT is there from the start, stored with the account so a restore knows its name.
            runCatching { add("YoGPT") }
            syncItem()
            refresh(); consumeInvite(); listen(); YoWork.registerIfConfigured(context)
        } finally { mutable.update { it.copy(busy = false) } }
    }
    suspend fun restore(codeText: String) = accountTurn.withLock {
        check(client == null)
        mutable.update { it.copy(busy = true) }
        try {
            val code = recoveryKeyParse(codeText) ?: recoveryKeyParse(codeText.lowercase().filter { it in 'a'..'z' || it in '0'..'9' }.takeLast(64))
                ?: error(context.getString(R.string.ui_that_doesn_t_match_a_recovery_key_check_it_and_try_again))
            val opened = openBackup(transport, null, YoDoor.RecoveryKey(code)) ?: error(context.getString(R.string.ui_we_couldn_t_find_your_account_check_your_recovery_key))
            restoreBackup(opened, code)
        } finally { mutable.update { it.copy(busy = false) } }
    }

    internal suspend fun restorePasskey(answer: Passkeys.Answer) = accountTurn.withLock {
        check(client == null)
        mutable.update { it.copy(busy = true) }
        try {
            val opened = openBackup(transport, answer.user, YoDoor.Passkey(answer.prf))
                ?: error(context.getString(R.string.no_passkey))
            restoreBackup(opened, null)
        } finally { mutable.update { it.copy(busy = false) } }
    }

    private fun registerCore(name: String, account: ByteArray): YoClient {
        val key = vault.secret("state-key")
        return YoClient.registerUsernameDb(transport, file.database(key), key, null, name, account, null, null, null, 0u, "")
    }

    private suspend fun restoreBackup(opened: YoOpenedBackup, typedCode: String?) {
        val sealed = JSONObject(opened.sealed.decodeToString())
        val account = sealed.getJSONObject("account")
        val c = registerCore(account.getString("username"), decode(account.getString("accountKey")))
        pin(c); client = c; sessionId++; vault.delete("left")
        val same = c.userId().hexBytes().contentEquals(decode(account.getString("userID")))
        val recovered = if (same) sealed.stringOrNull("code") ?: typedCode else null
        if (recovered != null) vault.write("recovery", recovered.toByteArray())
        mutable.update { it.copy(username = c.username(), secured = same, recovery = recovered, broken = false, friends = emptyList()) }
        persist(); refresh(); consumeInvite(); listen(); YoWork.registerIfConfigured(context)
        syncItem()
    }

    /**
     * Signs in with the synced item's account key, as iOS does with its iCloud Keychain item (`storageKey`: an old
     * install's, its storage still sealed under it). The item's backup counts only for the account it was made for: a
     * name registered anew after a deletion is another account (a kept sign-in of `migrate` names none).
     */
    private suspend fun signIn(item: SyncedItem, storageKey: ByteArray? = null) = accountTurn.withLock {
        check(client == null)
        val key = vault.secret("state-key")
        val c = YoClient.registerUsernameDb(transport, file.database(key), key, null, item.username, item.accountKey,
            item.legacySeed, storageKey, null, 0u, "")
        pin(c); client = c; sessionId++
        val same = item.userId.isEmpty() || c.userId().hexBytes().contentEquals(item.userId)
        val code = item.recoveryKey.takeIf { same }
        passkeyBackup = same && item.byPasskey
        if (code != null) vault.write("recovery", code.toByteArray()) else vault.delete("recovery")
        mutable.update { it.copy(username = c.username() ?: item.username, secured = code != null,
            recovery = code, broken = false, friends = emptyList()) }
        persist(); refresh(); consumeInvite(); listen(); YoWork.registerIfConfigured(context)
        syncItem()
    }

    /** The synced item for this account now: its key, the recovery key and how the backup opens. */
    private fun syncItem() {
        val c = client ?: return
        val code = state.value.recovery.takeIf { state.value.secured }
        val item = SyncedItem(c.username() ?: state.value.username.orEmpty(), c.userId().hexBytes(), c.accountSecret(),
            passkeyBackup, code != null, code != null, passkeyBackup, code, null, syncedItemVersion(), null)
        val cloud = runCatching { synced.write(item, state.value.googleBackup) }.getOrDefault(false)
        val missing = if (cloud || !state.value.googleBackup) null else synced.missing()
        if (c === client) mutable.update { it.copy(backedUp = cloud, backupMissingReason = missing) }
    }

    /** The person's "Back up to Google" choice: kept beside the account (`google-backup`), so it outlives a sign-out. */
    private fun backupWanted() = vault.read("google-backup") == null

    /** "Back up to Google": on only with end-to-end encryption (a screen lock); else it says why and stays off. */
    suspend fun googleBackup(on: Boolean) {
        changes.withLock {
            mutable.update { it.copy(googleBackup = on) }; persist()
            if (on) vault.delete("google-backup") else vault.write("google-backup", byteArrayOf(0))
        }
        syncItem()
        if (on && !state.value.backedUp) error(context.getString(
            if (state.value.backupMissingReason == CloudMissing.DEVICE_BACKUP) R.string.google_backup_needs_device_backup
            else R.string.google_backup_needs_screen_lock))
    }

    internal fun passkeyAccount(): Pair<String, ByteArray>? = client?.let { c ->
        c.username()?.let { it to c.userId().hexBytes() }
    }

    internal suspend fun keepPasskey(answer: Passkeys.Answer) = saveRecovery(answer)

    suspend fun prepareRecovery(renew: Boolean = false) {
        val c = client ?: return
        val code = if (renew) newRecoveryKey() else state.value.recovery ?: vault.read("recovery-pending")?.decodeToString() ?: newRecoveryKey()
        vault.write("recovery-pending", code.toByteArray())
        mutable.update { it.copy(recovery = code) }
        check(c === client)
    }
    suspend fun keepRecovery() = saveRecovery(null)

    internal suspend fun hasRecoveryPasskey(): Boolean = accountTurn.withLock {
        val code = vault.read("recovery")?.decodeToString() ?: return@withLock false
        val record = openBackup(transport, null, YoDoor.RecoveryKey(code))?.record ?: return@withLock false
        record.byPasskey.isNotEmpty() && record.authPasskey.isNotEmpty()
    }

    internal suspend fun removeRecoveryPasskey() = saveRecovery(null, removePasskey = true)

    private suspend fun saveRecovery(answer: Passkeys.Answer?, removePasskey: Boolean = false) = accountTurn.withLock {
        val c = client ?: return@withLock
        if (answer != null) check(answer.user.contentEquals(c.userId().hexBytes()))
        val old = vault.read("recovery")?.decodeToString()
        val code = if (removePasskey) checkNotNull(old) else state.value.recovery ?: if (answer != null) newRecoveryKey()
            else error(context.getString(R.string.ui_recovery_key_missing))
        val backup = old?.let { openBackup(transport, null, YoDoor.RecoveryKey(it)) }
        if (removePasskey) checkNotNull(backup)
        val passkey = if (removePasskey) YoPasskeyDoor.None else answer?.let { YoPasskeyDoor.New(it.prf) }
            ?: backup?.let { YoPasskeyDoor.Keep(it.record) } ?: YoPasskeyDoor.None
        val account = JSONObject().put("username", c.username()).put("userID", encode(c.userId().hexBytes()))
            .put("accountKey", encode(c.accountSecret())).put("byPasskey", !removePasskey && (answer != null || backup?.record?.byPasskey?.isNotEmpty() == true)).put("byCode", true)
            .put("codeAuth", true).put("passkeyAuth", !removePasskey && (answer != null || backup?.record?.authPasskey?.isNotEmpty() == true)).put("backupVersion", 3)
        val sealed = JSONObject().put("account", account).put("code", code).toString().toByteArray()
        c.putBackup(backup?.k ?: randomBytes(32), sealed, code, passkey)
        check(c === client)
        vault.write("recovery", code.toByteArray()); vault.delete("recovery-pending")
        mutable.update { it.copy(secured = true, recovery = code) }; changes.withLock { persist() }
        passkeyBackup = !removePasskey && (answer != null || backup?.record?.byPasskey?.isNotEmpty() == true)
        syncItem()
    }
    fun active(on: Boolean) {
        foreground = on
        present { foreground(it, on) }
        if (on && state.value.updating && !state.value.loading) keepResuming()
        else if (on) command {
            notifications.clear()
            listen()
            client?.let { source ->
                YoWork.registerIfConfigured(context)
                absorb(source.maintenance(), source)
            }
            media.wake()
            // Back from Android Settings with a screen lock or Google's backup turned on: the cloud copy follows at once.
            if (state.value.backupMissing) syncItem()
        } else {
            listening?.cancel()
            listening = null
        }
    }
    private fun listen() {
        if (!foreground || listening?.isActive == true || client == null || state.value.update) return
        listening = scope.launch(Dispatchers.IO) {
            var pause = 1000L
            while (isActive && foreground && !state.value.update) {
                val c = client ?: break
                try {
                    syncing.withLock {
                        absorb(c.savedEvents(), c)
                        val events = c.sync(25u)
                        if (c === client) absorb(events, c)
                    }
                    mutable.update { it.copy(lastSync = System.currentTimeMillis()) }
                    refresh(); media.wake(); pause = 1000
                } catch (e: CancellationException) { throw e }
                catch (e: YoException.Replaced) { signOut(replaced = true); break }
                catch (_: Exception) { withTimeoutOrNull(pause) { online.receive() }; pause = (pause * 2).coerceAtMost(30000) }
            }
        }
    }
    private var registeredPush: Pair<Long, String>? = null

    suspend fun registerPush(token: String) = accountTurn.withLock {
        val c = client ?: return@withLock
        val registration = sessionId to token
        if (registeredPush != registration) {
            c.setPush(3u, token)
            registeredPush = registration
        }
    }

    suspend fun syncPush() {
        if (foreground && listening?.isActive == true) return
        syncing.withLock {
            val c = client ?: return
            absorb(c.savedEvents(), c)
            absorb(c.sync(0u), c)
            if (c === client) {
                mutable.update { it.copy(lastSync = System.currentTimeMillis()) }
            }
        }
        // Keep the process eligible to complete downloads started by the inbox pass.
        withTimeoutOrNull(15_000) {
            state.first { snapshot -> snapshot.media.none { !it.ready } }
        }
    }

    suspend fun pushFallback() = changes.withLock {
        if (!foreground) {
            crashes.countNotificationFallback("pushSyncFailed")
            try {
                notifications.show("yo-fallback", "yo-fallback:${java.util.UUID.randomUUID()}", null, null, MessageKind.Yo)
            } catch (_: SecurityException) { }
        }
    }

    suspend fun sendWidgetYo(key: String?, account: String?, latest: Boolean) {
        val c = client ?: return
        if (!latest && account != c.userId()) return
        val snapshot = state.value
        val target = if (latest) snapshot.lastReceived.maxByOrNull { it.value }?.key else key
        val friend = snapshot.friends.firstOrNull { it.key == target && productWidgetEligible(it.blocked, it.request, it.gone) } ?: return
        sendNow(friend, c)
    }

    fun widgetAccount() = client?.userId()

    suspend fun add(typed: String) {
        val c = client ?: return
        val name = usernameCanonical(typed)
        require(name.isNotBlank()) { context.getString(R.string.type_name) }
        if (usernameSkeleton(name) == usernameSkeleton(state.value.username ?: "")) error(copy(ProductText.THATS_YOU))
        val found = try { c.whois(name) ?: error(copy(ProductText.NO_SUCH_USER)) } catch (e: YoException.Network) { null }
        if (c !== client) return
        changes.withLock {
            val spelled = found?.username ?: name
            val known = state.value.friends.firstOrNull { it.group == null && !it.gone &&
                ((found != null && it.user == found.user) || usernameSkeleton(it.name) == usernameSkeleton(spelled)) }
            if (known != null) { showSent(known, copy(ProductText.ALREADY_ADDED)); return@withLock }
            found?.user?.let { removedUsers.remove(it) }
            val friend = Friend(name = spelled, color = nextColor(state.value.friends), user = found?.user)
            if (friend.user != null) c.setFriend(friend.user, friend.name, friend.color.toUByte())
            mutable.update { it.copy(friends = listOf(friend) + it.friends) }; listFriends(); persist()
        }
        c.writeStorage()
    }
    suspend fun invite(name: String) {
        mutable.update { it.copy(pendingInvite = name) }; changes.withLock { persist() }; consumeInvite()
    }
    private suspend fun consumeInvite() {
        if (client == null) return
        val name = state.value.pendingInvite ?: return
        // Taken once, whatever the answer: a name not on Yo says so and never comes back at the next start
        // (where it failed the start: "Can't Open Your Account"), at sign-up or sign-in.
        mutable.update { it.copy(pendingInvite = null) }; changes.withLock { persist() }
        if (usernameSkeleton(name) == usernameSkeleton(state.value.username ?: "")) return
        try { add(name) } catch (e: CancellationException) { throw e } catch (e: Exception) {
            if (e is YoException.Replaced) throw e
            mutable.update { it.copy(error = friendlyError(context, e)) }
        }
    }
    suspend fun send(friend: Friend) {
        val c = client ?: return
        if (friend.gone) { showSent(friend, copy(ProductText.NOT_ON_YO)); return }
        click()
        if (friend.undo) { media.holdYo(friend); return }
        sendNow(friend, c)
    }
    suspend fun sendNow(friend: Friend, c: YoClient) {
        val lock = synchronized(sending) { sending.getOrPut(friend.target) { Mutex() } }
        lock.withLock {
            if (c !== client) return
            try {
                val events = c.send(sendTarget(friend.target), MessageKind.Yo)
                absorb(events, c)
                showSent(friend)
                recordStat(friend,true)
                changes.withLock { persist() }
                if (vault.read("first-yo") == null) {
                    vault.write("first-yo", byteArrayOf(1)); mutable.update { it.copy(firstYo = true) }
                }
                deliverLater(c)
            } catch (e: Exception) {
                showSent(friend, when (e) {
                    is YoException.NotOnYo -> copy(ProductText.NOT_ON_YO)
                    is YoException.GroupEmpty -> copy(ProductText.GROUP_EMPTY)
                    is YoException.Unconfirmed -> copy(ProductText.CONFIRM_NEW_KEY)
                    is YoException.Blocked -> copy(ProductText.BLOCKED)
                    is YoException.Outdated -> copy(ProductText.NEEDS_UPDATE)
                    else -> copy(ProductText.NOT_SENT)
                })
                if (e is YoException.Replaced) throw e
                deliverLater(c)

            }
        }
    }
    private fun deliverLater(c: YoClient) {
        if (retrying?.isActive == true) return
        retrying = scope.launch(Dispatchers.IO) {
            var pause = 1000L
            while (c === client && !state.value.update) {
                try { if (c.waiting() == 0u) break; absorb(c.deliver(), c) } catch (_: Exception) { }
                delay(pause); pause = (pause * 2).coerceAtMost(30000)
            }
        }
    }
    /** A send's end on the bar: `sent` (Yo) or a failure's words from the core. */
    fun showSent(friend: Friend, failure: String? = null) {
        listFriends()
        present { time -> if (failure == null) sendFinished(time, friend.key, ListKind.YO) else sendFailed(time, friend.key, failure) }
    }
    private fun recordStat(friend: Friend,sent: Boolean) {
        val time=System.currentTimeMillis()
        mutable.update { s -> s.copy(sent=s.sent+if(sent) 1 else 0,received=s.received+if(sent) 0 else 1,
            byFriend=s.byFriend+(friend.key to ((s.byFriend[friend.key] ?: 0)+1)),
            lastSent=if(sent) s.lastSent+(friend.key to time) else s.lastSent,
            lastReceived=if(sent) s.lastReceived else s.lastReceived+(friend.key to time)) }
    }
    private val notifications = YoNotifications(context)
    private fun arrival(friend: Friend, user: String?, kind: MessageKind, token: String) {
        present { received(it, friend.key, when(kind) {
            is MessageKind.Photo -> ListKind.PHOTO
            is MessageKind.Video -> ListKind.VIDEO
            else -> ListKind.YO
        }, token, if(friend.group != null) user else null) }
        recordStat(friend,false)
        if (foreground) chime()
    }
    suspend fun refresh() = changes.withLock {
        val c = client ?: return@withLock
        val contacts = c.contacts()
        var friends = state.value.friends
        for (kept in c.friends()) {
            val old = friends.firstOrNull { it.user == kept.user }
            val contact = contacts.firstOrNull { it.user == kept.user }
            val name = kept.name ?: contact?.handles?.firstOrNull() ?: old?.name ?: "Yo User"
            val row = old?.copy(name = name, blocked = kept.blocked, color = kept.color?.toInt() ?: old.color)
                ?: Friend(name = name, color = kept.color?.toInt() ?: nextColor(friends), user = kept.user, blocked = kept.blocked)
            friends = if (old == null) friends + row else friends.map { if (it.key == row.key) row else it }
        }
        for (contact in contacts.filter { it.user !in removedUsers }) {
            val old = friends.firstOrNull { it.user == contact.user } ?: friends.firstOrNull { it.group == null && it.user == null && contact.handles.any { h -> usernameSkeleton(h) == usernameSkeleton(it.name) } }
            val row = (old ?: Friend(name = contact.handles.firstOrNull() ?: contact.name ?: "Yo User", color = nextColor(friends), user = contact.user))
                .copy(user = contact.user, blocked = contact.blocked, gone = contact.gone, request = !contact.accepted)
            friends = if (old == null) friends + row else friends.map { if (it.key == old.key) row else it }
        }
        val groups = c.groups()
        friends = friends.filter { f -> f.group == null || groups.any { it.id == f.group && !it.left } }
        for (group in groups.filter { !it.left }) {
            val old = friends.firstOrNull { it.group == group.id }
            val row = (old ?: Friend(name = group.name, color = nextColor(friends), group = group.id))
                .copy(name = group.name.ifEmpty { copy(ProductText.GROUP) }, request = !group.accepted)
            friends = if (old == null) listOf(row) + friends else friends.map { if (it.key == old.key) row else it }
        }
        mutable.update { it.copy(friends = friends, requestsOpen = c.requestsOpen()) }; listFriends(); persist()
    }
    suspend fun absorb(events: List<YoEvent>, source: YoClient? = client) {
        if (events.isEmpty() || source == null || source !== client) return
        changes.withLock {
            if (source !== client) return
            for (event in events) when (event) {
                is YoEvent.Message -> removedUsers.remove(event.user)
                is YoEvent.NewChat -> removedUsers.remove(event.user)
                else -> Unit
            }
        }
        refresh()
        val c = source
        changes.withLock {
            if (c !== client) return
            val kept = mutableListOf<YoEvent>()
            val profiles = events.filterIsInstance<YoEvent.Profile>().associate { it.user to it.name }
            val contacts = c.contacts().associateBy { it.user }
            for (event in events) {
                if(event is YoEvent.Profile) present { senderNamed(it,event.user,event.name) }
                val user = when (event) { is YoEvent.Message -> event.user; is YoEvent.GroupMessage -> event.user; is YoEvent.IdentityChanged -> event.user; else -> null }
                val group = (event as? YoEvent.GroupMessage)?.group
                val friend = state.value.friends.firstOrNull { if (group != null) it.group == group else user != null && it.user == user }
                if (event is YoEvent.IdentityChanged && friend != null) {
                    mutable.update { s -> s.copy(friends = s.friends.map { if (it.key == friend.key) it.copy(keyChanged = true) else it }, notice = "${friend.name} Has a New Key. Compare safety numbers in Profile › Encryption.") }
                }
                val at = when (event) { is YoEvent.Message -> event.sentAtMs; is YoEvent.GroupMessage -> event.sentAtMs; else -> null }
                val kind = when (event) { is YoEvent.Message -> event.kind; is YoEvent.GroupMessage -> event.kind; else -> null }
                if (friend != null && at != null && kind != null && !friend.blocked) {
                    val id = "${friend.target}/$user/$at"
                    // First save the media descriptor; an unsuccessful save leaves the event in the Rust journal.
                    media.expect(id, friend.target, user ?: "", kind, at.toLong())
                    if (seen.add(id)) {
                        if (seen.size > 2000) seen.remove(seen.first())
                        arrival(friend, user, kind, id)
                        if (user != null) {
                            val contact = contacts[user]
                            val name = profiles[user]?.takeIf { it.isNotBlank() }
                                ?: contact?.name?.takeIf { it.isNotBlank() }
                                ?: contact?.handles?.firstOrNull()
                                ?: state.value.friends.firstOrNull { it.user == user }?.name?.takeUnless { it == "Yo User" }
                            if (name == null) {
                                crashes.countNotificationFallback(if (group == null) "unknownSender" else "unknownGroupSender")
                            }
                            try {
                                if (!foreground) notifications.show(friend.target, id, name, friend.name.takeIf { group != null }, kind)
                            } catch (_: SecurityException) {
                                // Notification permission may be revoked between checking and posting.
                            }
                        }
                    }
                }
                kept += event
            }
            persist(); c.acknowledge(kept)
        }
        media.wake()
    }
    fun click() {
        val audio = context.getSystemService(android.media.AudioManager::class.java)
        if(audio.ringerMode == android.media.AudioManager.RINGER_MODE_NORMAL)
            audio.playSoundEffect(android.media.AudioManager.FX_KEY_CLICK)
    }
    private fun chime() {
        // MediaPlayer callbacks need a looper, even though the inbox runs on IO.
        scope.launch(Dispatchers.Main.immediate) {
            synchronized(soundPlayers) {
                if (soundPlayers.size >= productSoundLimit().toInt()) return@synchronized
                val attributes = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION).build()
                val player = MediaPlayer.create(context, R.raw.yo, attributes, 0) ?: return@synchronized
                fun release() = synchronized(soundPlayers) {
                    if (soundPlayers.remove(player)) player.release()
                }
                soundPlayers += player
                player.setOnCompletionListener { release() }
                player.setOnErrorListener { _, _, _ -> release(); true }
                try { player.start() } catch (_: IllegalStateException) { release() }
            }
        }
    }
    suspend fun answer(friend: Friend, yes: Boolean, block: Boolean = false) {
        val c = client ?: return
        if (yes) absorb(friend.group?.let { c.acceptGroup(it) } ?: c.accept(friend.target), c)
        else if (friend.group != null) absorb(c.leaveGroup(friend.group), c)
        else if (block) c.setBlocked(friend.target, true) else c.decline(friend.target)
        if (!yes && !block) remove(friend)
        refresh()
    }
    suspend fun remove(friend: Friend) {
        friend.user?.let { client?.removeFriend(it) }
        changes.withLock { friend.user?.let { removedUsers.add(it) }; mutable.update { it.copy(friends = it.friends.filter { f -> f.key != friend.key }) }; listFriends(); persist() }
        client?.writeStorage()
    }
    suspend fun block(friend: Friend, blocked: Boolean) { client?.setBlocked(friend.target, blocked); refresh(); client?.writeStorage(); media.wake() }
    suspend fun report(friend: Friend, reason: ReportReason) { client?.report(friend.target, reason) }
    suspend fun createGroup(name: String, members: List<String>) {
        val c = client ?: return
        val made = c.createGroup(utf8Limit(name.trim(), 32), members.take(19))
        absorb(made.events, c); if (c !== client) return; refresh()
        val names=members.mapNotNull { id -> state.value.friends.firstOrNull { it.user==id }?.name }
        val shown=if(names.size>4) names.take(3) else names
        val listed=shown+if(shown.size<names.size) listOf(context.resources.getQuantityString(R.plurals.more_members,names.size-shown.size,names.size-shown.size)) else emptyList()
        val line=context.getString(R.string.ui_made,state.value.username.orEmpty(),name.trim())
        val sentence=if(listed.isEmpty()) line else context.getString(R.string.with_members,line,android.icu.text.ListFormatter.getInstance().format(listed))
        groupNotice(made.id,sentence,names,listOf(name.trim()))
    }
    suspend fun groupMembers(friend: Friend) = client?.groups()?.firstOrNull { it.id == friend.group }?.members.orEmpty()
    suspend fun groupChange(friend: Friend, rename: String? = null, add: List<String> = emptyList(), remove: List<String> = emptyList(), leave: Boolean = false) {
        val c = client ?: return
        val group = friend.group ?: return
        if (leave) {
            val id = groupNotice(group, context.getString(R.string.ui_left,state.value.username.orEmpty(),friend.name),emptyList(),listOf(friend.name))
            repeat(60) { if (media.isWaiting(id)) delay(500) }
        }
        val events = when {
            leave -> c.leaveGroup(group)
            rename != null -> c.renameGroup(group, utf8Limit(rename.trim(), 32))
            add.isNotEmpty() -> c.addToGroup(group, add)
            else -> c.removeFromGroup(group, remove)
        }
        absorb(events, c); if (c !== client) return; refresh()
        if(rename!=null) groupNotice(group,context.getString(R.string.ui_renamed_the_group_to,state.value.username.orEmpty(),rename),emptyList(),listOf(rename))
        else if(!leave) (add+remove).forEach { id ->
            val name=state.value.friends.firstOrNull { it.user==id }?.name ?: context.getString(R.string.group_unknown_member)
            groupNotice(group,context.getString(if(id in add) R.string.member_added else R.string.member_removed,state.value.username.orEmpty(),name,friend.name),listOf(name),listOf(friend.name))
        }
    }
    private suspend fun groupNotice(group: String,text: String,users: List<String>,groups: List<String>): String =
        media.stageCard(context,"group:$group",text,users+state.value.username.orEmpty(),groups)
    suspend fun feedback(text: String) { client?.sendSupport(utf8Limit(text.trim(), 2000), BuildConfig.VERSION_NAME) }
    suspend fun supportThread() = client?.supportThread()
    suspend fun supportRead(through: ULong) { client?.supportRead(through) }
    suspend fun roundTrip() = client?.roundTripProbe("PINGTEST", 10000u)
    val crashes = CrashReports(context, transport)

    suspend fun settings(left: Boolean = state.value.leftHanded, save: Boolean = state.value.saveSent, requests: Boolean = state.value.requestsOpen) {
        if (requests != state.value.requestsOpen) client?.setRequestsOpen(requests)
        changes.withLock { mutable.update { it.copy(leftHanded = left, saveSent = save, requestsOpen = requests) }; persist() }
    }
    suspend fun undoSettings(friend: Friend, on: Boolean, seconds: Int) = changes.withLock {
        mutable.update { it.copy(friends = it.friends.map { f -> if (f.key == friend.key) f.copy(undo = on, undoSeconds = productUndoSeconds(seconds.toDouble()).toInt()) else f }) }; persist()
    }
    suspend fun security() = client?.securityStatus()
    suspend fun contact(friend: Friend): ContactDetails? {
        val c=client ?: return null
        val contact=c.contacts().firstOrNull { it.user==friend.user }
        return ContactDetails(contact,friend.user?.let { c.accountKeyOf(it) },
            friend.user?.takeIf { contact!=null }?.let { c.safetyNumber(it) },friend.user?.let { c.sessionsWith(it) }.orEmpty(),
            friend.user?.takeIf { contact!=null }?.let { c.safetyCode(it) })
    }
    /** Verified only when the code scanned off the contact's screen matches (spec 3.4). */
    suspend fun verifyScan(friend: Friend, scanned: ByteArray): Boolean {
        val c = client ?: return false
        if (!c.safetyCodeMatches(friend.target, scanned)) return false
        c.setVerified(friend.target, true); refresh(); return true
    }
    suspend fun verify(friend: Friend, verified: Boolean) { client?.setVerified(friend.target, verified); refresh(); media.wake() }
    suspend fun confirm(friend: Friend) { client?.confirmIdentity(friend.target); refresh(); media.wake(); client?.let { deliverLater(it) } }
    suspend fun icon(name: String?) {
        val aliases = mapOf<String?,String>(null to "Default", "Negative" to "Negative", "Original" to "Original")
        val selected = aliases[name] ?: return
        val manager = context.packageManager
        fun component(value: String) = android.content.ComponentName(context.packageName, "app.justyo.android.${value}Icon")
        manager.setComponentEnabledSetting(component(selected), android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_ENABLED, android.content.pm.PackageManager.DONT_KILL_APP)
        aliases.values.filter { it != selected }.forEach { value -> manager.setComponentEnabledSetting(component(value), android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_DISABLED, android.content.pm.PackageManager.DONT_KILL_APP) }
        client?.setAppIcon(name)
        mutable.update { it.copy(appIcon = name) }
        changes.withLock { persist() }
        client?.writeStorage()
    }
    suspend fun saveContacts() { client?.writeStorage(); mutable.update { it.copy(notice = context.getString(R.string.contacts_saved)) } }
    suspend fun deleteAccount() {
        val c = client ?: return
        c.deleteAccount()
        signOut(change = "delete")
    }
    /**
     * `byPerson`: the person signed out, so the synced item is marked (no automatic sign-in until they sign in);
     * `change`: what Block Store is to do, kept until it did (`settleSynced`). `replaced`: another phone took the
     * account, which this one never takes back by itself (iOS: `SyncedAccount.left`).
     */
    suspend fun signOut(byPerson: Boolean = false, replaced: Boolean = false, change: String? = "signedOut".takeIf { byPerson }) = accountTurn.withLock {
        client?.userId()?.let { user ->
            if (change != null) vault.write("synced-pending", "$change $user".toByteArray())
            if (replaced) vault.write("left", user.hexBytes())
        }
        notifications.clear()
        val old = client; client = null; sessionId++
        listening?.cancel(); listening = null; retrying?.cancel(); retrying = null
        if (registeredPush?.first == sessionId - 1) runCatching { old?.setPush(0u, "") }
        registeredPush = null
        old?.shutdown()
        changes.withLock {
            media.clear(); file.delete(vault.read("state-key")); vault.clear(); seen.clear(); removedUsers.clear(); clearTemporaryMedia()
            synchronized(listLock) {
                listJob?.cancel();orderSave?.cancel();displayedOrder=emptyList();list=YoList();mutable.value=AppState(loading=false, googleBackup=backupWanted())
            }
        }
        settleSynced()
    }
    private fun clearTemporaryMedia() {
        context.cacheDir.listFiles().orEmpty().filter { file ->
            listOf("view-", "capture-", "prepared-").any { prefix -> file.name.startsWith(prefix) }
        }.forEach { it.delete() }
    }
    private suspend fun refreshMedia() {
        val received = media.received()
        mutable.update { it.copy(media = received) }
        present { time ->
            state.value.friends.forEach { friend -> mediaChanged(time,friend.key,
                received.filter { it.target == friend.target }.map { if(it.video) ListKind.VIDEO else ListKind.PHOTO }) }
            tick(time)
        }
    }
    fun clearMessage(message: String) { mutable.update { it.copy(
        error=it.error.takeUnless { text -> text==message },notice=it.notice.takeUnless { text -> text==message }) } }
    fun clearError() { mutable.update { it.copy(error = null, notice = null, firstYo = false) } }
    companion object {
        /** A failed one-time sign-in that is tried again later, not dropped: a network error, a busy hub, a save. */
        fun keepsPendingSignIn(error: Exception) = error is YoException && retryLater(error)

        /** An old install's sign-in to keep (`migrate`): its name, and its account key or else its recovery key. */
        fun pendingSignIn(username: String?, identity: LegacyIdentity?, recovery: String?): SyncedItem? {
            if (username == null || (identity == null && recovery == null)) return null
            val seed = identity?.ed25519 == true
            return SyncedItem(username, byteArrayOf(), identity?.secret ?: byteArrayOf(), false, recovery != null, null,
                null, recovery, null, if (seed) null else syncedItemVersion(), null)
        }
        fun nextColor(friends: List<Friend>): Int = (0..7).minBy { color -> friends.count { it.color % 8 == color } }
        fun decodeUrl(text: String): ByteArray = Base64.decode(text, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
        fun encode(data: ByteArray): String = Base64.encodeToString(data, Base64.NO_WRAP)
        fun decode(text: String): ByteArray = Base64.decode(text, Base64.DEFAULT)
        // Public dev hub parameters copied from Hub.swift. No private key is shipped.
        const val DEV_GROUP_PARAMS = "AABte-FyyNG7G0rm_Swi9dLOf6khh32C0-TmfHELCW5J4k4m_beuhn-LDZ6GFT19xzzza_-DQfEKUPdsKy99Knl4FgkgVTu8j8yZP5JoagLdul10PyCTaUcx649Z-JJuUWidD_T_ZYOgvSS61z9QixJQB6LAB_k0AYFY563qzWItEncY_vD68l2OUM1aTDNuXO6QeT0MgeRwrlsqLhF10C2SMQJotdkaEOGAMdr4E261qG0CNuVEua6ojEXs7BXCOpTIPvZhkxQf6N2-WWXVXyrm65kQSDkE32SYFSTxhP9dNNdVop_SDJE24rMYPJy1SH5DD1gs7Y67w0RNQIooUHmaUn3WlOjHXFMgwHpDDMGlDuMqgEMkz1D-ArhgAoYsXG6LduuWJgSRWi3KED6jZRm2QCUpd_LIkZKR2vmQp_Y68IvRf_myayJE33VuqTbfT0jp9KBeiJuIOdqubZBWb2vG_jI0ihgyFRJPTrFtpW0iv1tpngyN0m7_V3I-jUlzIGQSJRJDvDvlEvKPMDeA9Sc2Nh5tRY3JiMTqcFwCVMsbCujL_mhC4VBfXUFy-urGdLiqdTBZF53mInuT_yR8VFoSPLeTi7mHnRSlACAnh_GFnE80FlbXVEbW8EdMXzwycwLy3GajU4z0wOLmeI58jYlwZw8A-dNcB2CHG4zBwpkTiD2ZHZcdAL4kSljBUqE2Tml9cfKtGzqcvH7zrxOhjCL6Sn9IyV36TVf6lKf-oRjDHsPHP9pSp3apVZfG1dEdRQr2UEvHRHla3C8jQrte8e02zrqQwG0KxsUTueOVgiVsWtaUV3YohIHtxbSUOljwoaA-iRmPQsvUHgcpL2UoOghIMYvKSbwRIde_-NYhyQVSCBpcgz5bhRbHcc-uS_j7Qg"
    }
}
class ContactDetails(val contact: YoContact?, val accountKey: String?, val safetyNumber: String?, val sessions: List<YoSession>,
    /** The safety number's QR code payload (Signal's scannable form). */
    val safetyCode: ByteArray?)
fun String.hexBytes() = chunked(2).map { it.toInt(16).toByte() }.toByteArray()
fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }
fun ByteArray.sha256Hex(): String = java.security.MessageDigest.getInstance("SHA-256").digest(this).joinToString("") { "%02x".format(it) }
fun friendlyError(context: Context,error: Exception): String = when (error) {
    is YoException.Network -> context.getString(R.string.ui_yo_can_t_reach_its_server_check_your_connection_and_try_again)
    is YoException.Refused -> when (error.status.toInt()) {
        409 -> context.getString(R.string.ui_that_name_is_taken_try_another)
        429 -> context.getString(R.string.ui_too_many_tries_try_again_later)
        400 -> context.getString(R.string.ui_check_the_name_or_choose_a_longer_one)
        else -> context.getString(R.string.hub_refused,error.status.toInt())
    }
    is YoException.State -> context.getString(R.string.account_unavailable)
    else -> error.message?.takeIf { it.isNotBlank() } ?: context.getString(R.string.generic_failure)
}
