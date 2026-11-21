package app.justyo.android

import android.content.Context
import android.media.AudioAttributes
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
    val transport = HubTransport(reached = { connection.reached(it) }) { mutable.update { it.copy(update = true) } }
    /** Offline as iOS says it: no network, or the hub not reached, for 4 s on end (a first second without is nothing);
     *  else what an action ran into (`Problems`), a tap on it tries again. */
    private inner class Connection {
        @Volatile var local = false
        @Volatile var remote = false
        @Volatile var offline = false
        private var pending: Job? = null
        fun reached(ok: Boolean) {
            // Any answer of the hub counts for Status' "Connected" (a long poll answers only after 25 s).
            if (ok) { val now = System.currentTimeMillis(); if (now - (state.value.lastSync ?: 0) > 5000) mutable.update { it.copy(lastSync = now) } }
            if (remote != ok) return; remote = !ok; settle()
        }
        fun network(available: Boolean) { local = !available; if (available) remote = false; settle() }
        @Synchronized fun settle() {
            if (!local && !remote) { pending?.cancel(); pending = null; if (offline) { offline = false; show() }; return }
            if (offline || pending?.isActive == true) return
            pending = scope.launch { delay(4000); offline = local || remote; show() }
        }
        @Synchronized fun foreground() { remote = false; pending?.cancel(); pending = null; offline = false; settle(); show() }
        fun show() = present { connectivity(it, offline, Problems.text.value) }
    }
    private val connection by lazy { Connection() }
    private var backgroundSince = 0L
    @Volatile private var client: YoClient? = null
    @Volatile var sessionId: Long = 0
        private set
    private val changes = Mutex()
    internal val pendingShares=PendingShares(context,vault,changes) { sessionId }
    private val accountTurn = Mutex()
    private val sending = mutableMapOf<String, Mutex>()
    /** Yos that did not go, by target (`unsent-yos`): the bar says "Not Sent · Tap to Retry" until a tap sent them. */
    private val unsentYos = mutableMapOf<String, Int>()
    /** Targets with media that did not go (`MediaQueue.unsent`), under the same lock. */
    private val mediaUnsent = mutableSetOf<String>()
    private var list = YoList()
    private var listJob: Job? = null
    private val listLock = Any()
    private var orderSave: Job? = null
    private var displayedOrder=emptyList<String>()
    private fun now() = android.os.SystemClock.elapsedRealtime().toULong()
    /**
     * A row's clock runs while the row is seen, and it shows only in a fade (a received Yo). For a
     * row without one, scrolling it in or out of view changes nothing on screen: it keeps its old value, so the state
     * stays equal and neither the list nor the widget recomposes on every scroll.
     */
    private fun steadyRows(old: Map<String, ListRow>, rows: List<ListRow>) = rows.associate { row ->
        val previous = old[row.id]
        row.id to (previous?.takeIf {
            row.fadeElapsed == null && it.clockRunning != row.clockRunning &&
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
            state.copy(friends=ordered,listRows = steadyRows(state.listRows, snapshot.rows), lockedRow = snapshot.lockedRow,deletion=snapshot.deletion,banner=snapshot.banner)
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
            state.value.friends.firstOrNull { it.key==id }?.let { friend -> command { remove(friend) } }
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
        queue.unsent = { target, on -> synchronized(unsentYos) { if (on) mediaUnsent += target else mediaUnsent -= target }; markUnsent(target) }
        queue.sent = { target, last, video ->
            state.value.friends.firstOrNull { it.target == target }?.let {
                // As a photo or video: sent while the camera is open, the bar keeps saying so until it closes (core).
                if (last) showSent(it, kind = if (video) ListKind.VIDEO else ListKind.PHOTO)
                recordStat(it, true)
            }
        }
    }
    init {
        runCatching {
            context.getSystemService(android.net.ConnectivityManager::class.java)
                .registerDefaultNetworkCallback(object : android.net.ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: android.net.Network) {
                        // Another network: connections of the one before cannot carry anything any more.
                        transport.forgetConnections()
                        connection.network(true)
                        online.trySend(Unit)
                        if (client != null && state.value.pendingInvite != null) command { consumeInvite() }
                        if (foreground && client != null) media.wake()
                    }
                    override fun onLost(network: android.net.Network) { connection.network(false) }
                })
        }
    }
    fun start() = scope.launch(Dispatchers.IO) {
        // An action's problem in the banner, and gone from it again.
        scope.launch { Problems.text.collect { connection.show() } }
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
                recoverySaved = cached.optBoolean("recoverySaved", true), recoveryPasskey = cached.optBoolean("recoveryPasskey"),
                sent = cached.optLong("sent"), received = cached.optLong("received"),
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
                loadUnsent()
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
        c.setHubKeys(Hub.hubKeys.map(::decodeUrl))
        c.setTrustRoots(Hub.roots.map(::decodeUrl))
        c.setZkParams(decodeUrl(Hub.groupParams))
    }
    private suspend fun persist() {
        val s = state.value
        vault.write("ui", JSONObject().put("username", s.username).put("friends", JSONArray(s.friends.map { it.json() }))
            .put("byFriend",JSONObject(s.byFriend)).put("lastSent",JSONObject(s.lastSent)).put("lastReceived",JSONObject(s.lastReceived))
            .put("appIcon", s.appIcon).put("secured", s.secured).put("leftHanded", s.leftHanded)
            .put("recoverySaved", s.recoverySaved).put("recoveryPasskey", s.recoveryPasskey)
            .put("sent", s.sent).put("received", s.received).put("invite", s.pendingInvite).put("removed", JSONArray(removedUsers.toList()))
            .put("googleBackup", s.googleBackup).toString().toByteArray())
    }
    /** A user action: what the hub did not do goes to the banner (`Problems`); this phone's own words to the snackbar,
     *  and before sign-in everything to the onboarding note. */
    fun command(work: suspend Repository.() -> Unit) = scope.launch(Dispatchers.IO) {
        try { work() } catch (e: CancellationException) { throw e } catch (e: Exception) { failed(e) }
    }
    private suspend fun failed(e: Exception) {
        if (e is YoException.Replaced) signOut(replaced = true)
        else if (client != null && (e is YoException || e is java.io.IOException)) Problems.report(e)
        else mutable.update { it.copy(error = friendlyError(context,e)) }
    }
    /** The account's contacts and settings to the hub; one not taken is said, and a tap on the banner tries again. */
    private fun store(c: YoClient) {
        try { c.writeStorage() } catch (e: Exception) {
            if (e is YoException.Replaced) throw e
            Problems.report(e) { command { client?.writeStorage() } }
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

    /** The account item Block Store holds, for the Sign In card; none when there is none or Block Store did not answer. */
    suspend fun savedAccount(): SyncedItem? = withContext(Dispatchers.IO) { runCatching { synced.read() }.getOrNull() }

    /** The Sign In card's account tapped: also one signed out here (iOS: `restore(synced:)` clears `left`). */
    suspend fun signInSaved(item: SyncedItem) {
        signIn(item)
        vault.delete("left")
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
        mutable.update { it.copy(username = c.username(), secured = same, recovery = recovered, recoverySaved = true, broken = false, friends = emptyList()) }
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
            recovery = code, recoverySaved = true, broken = false, friends = emptyList()) }
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

    suspend fun prepareRecovery() {
        val c = client ?: return
        val code = state.value.recovery ?: vault.read("recovery-pending")?.decodeToString() ?: newRecoveryKey()
        vault.write("recovery-pending", code.toByteArray())
        mutable.update { it.copy(recovery = code) }
        check(c === client)
    }
    suspend fun keepRecovery() = saveRecovery(null)

    /** The new key after "Turn Off Recovery Key" saved (shared or copied): the hub has it already. */
    suspend fun markRecoverySaved() {
        mutable.update { it.copy(recoverySaved = true) }; changes.withLock { persist() }
    }

    /**
     * "Turn Off Recovery Key" (iOS `newRecoveryKey`): the backup is rewrapped at once under a new key, same K and
     * passkey door, so the saved key stops working now; the new one shows "Not Saved" until it is saved. The new key
     * is staged first (`recovery-pending`): if the hub's answer is lost, the next try opens the backup with it.
     */
    suspend fun turnOffRecovery() = accountTurn.withLock {
        val c = client ?: return@withLock
        val (backup, _) = listOfNotNull(vault.read("recovery"), vault.read("recovery-pending")).map { it.decodeToString() }
            .firstNotNullOfOrNull { key -> openBackup(transport, null, YoDoor.RecoveryKey(key))?.let { it to key } }
            ?: error(context.getString(R.string.ui_recovery_key_missing))
        check(c === client)
        val code = newRecoveryKey()
        vault.write("recovery-pending", code.toByteArray())
        val sealed = JSONObject(backup.sealed.decodeToString()).put("code", code)
        sealed.optJSONObject("account")?.put("byCode", true)?.put("codeAuth", true)
        c.putBackup(backup.k, sealed.toString().toByteArray(), code, YoPasskeyDoor.Keep(backup.record))
        check(c === client)
        vault.write("recovery", code.toByteArray()); vault.delete("recovery-pending")
        val passkey = backup.record.byPasskey.isNotEmpty() && backup.record.authPasskey.isNotEmpty()
        mutable.update { it.copy(recovery = code, recoverySaved = false, recoveryPasskey = passkey) }; changes.withLock { persist() }
        syncItem()
    }
    /** The recovery key this phone saved, to show it again. */
    internal fun savedRecovery(): String? = vault.read("recovery")?.decodeToString()

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
        passkeyBackup = !removePasskey && (answer != null || backup?.record?.byPasskey?.isNotEmpty() == true)
        val passkeyOn = !removePasskey && (answer != null || backup?.record?.authPasskey?.isNotEmpty() == true) && passkeyBackup
        mutable.update { it.copy(secured = true, recovery = code, recoveryPasskey = passkeyOn,
            recoverySaved = it.recoverySaved || answer == null && !removePasskey) }; changes.withLock { persist() }
        syncItem()
    }
    /** While Yo is in front, Wi-Fi stays out of power save: an idle phone's radio otherwise sleeps between beacons
     *  and a Yo's packets wait at the access point (measured up to 400 ms on the A34). Android ends it by itself
     *  when the screen goes off or Yo leaves the foreground. */
    private val lowLatency = if (android.os.Build.VERSION.SDK_INT < 29) null else runCatching {
        context.getSystemService(android.net.wifi.WifiManager::class.java)
            .createWifiLock(android.net.wifi.WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "yo:foreground")
            .apply { setReferenceCounted(false) }
    }.getOrNull()
    fun active(on: Boolean) {
        foreground = on
        runCatching { if (on) lowLatency?.acquire() else lowLatency?.release() }
        if (on) {
            // Back after a while: the hub's connections may be gone (the phone slept, the network moved on).
            if (backgroundSince != 0L && android.os.SystemClock.elapsedRealtime() - backgroundSince > 20_000) transport.forgetConnections()
            connection.foreground()
        } else backgroundSince = android.os.SystemClock.elapsedRealtime()
        if (on) { preloadSound(); refreshSupport() }
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
                        Lat.mark("sync.wait")
                        val events = c.sync(25u)
                        Lat.mark("sync.got ${events.size}")
                        if (c === client) absorb(events, c)
                        Lat.mark("sync.absorbed")
                        // absorb refreshed the list for events; an empty answer may still have changed the core's lists
                        if (events.isEmpty()) refresh()
                    }
                    mutable.update { it.copy(lastSync = System.currentTimeMillis()) }
                    if (c.waiting() == 0u) Problems.resolve(ProductProblem.WAITING)
                    media.wake(); pause = 1000
                } catch (e: CancellationException) { throw e }
                catch (e: YoException.Replaced) { signOut(replaced = true); break }
                catch (_: Exception) { withTimeoutOrNull(pause) { online.receive() }; pause = (pause * 2).coerceAtMost(30000) }
            }
        }
    }
    private var registeredPush: Pair<Long, String>? = null
    /** The session whose FCM token the hub has: Status says Ready for it, No Token otherwise (as iOS). */
    val pushSession = MutableStateFlow<Long?>(null)

    suspend fun registerPush(token: String, again: Boolean = false) = accountTurn.withLock {
        val c = client ?: return@withLock
        val registration = sessionId to token
        if (again || registeredPush != registration) {
            c.setPush(3u, token)
            registeredPush = registration
        }
        pushSession.value = registration.first
    }

    /** Settings › Status › Renew Push Registration (as Signal's): a new FCM token, sent to the hub even if it has the
     *  old one. Throws what failed (Firebase, the connection, the hub) for the red banner. */
    suspend fun renewPush() = withContext(Dispatchers.IO) {
        if (com.google.firebase.FirebaseApp.getApps(context).isEmpty()) error("no Firebase in this build")
        val messaging = com.google.firebase.messaging.FirebaseMessaging.getInstance()
        com.google.android.gms.tasks.Tasks.await(messaging.deleteToken(), 20, java.util.concurrent.TimeUnit.SECONDS)
        registerPush(com.google.android.gms.tasks.Tasks.await(messaging.token, 20, java.util.concurrent.TimeUnit.SECONDS), again = true)
    }

    suspend fun syncPush() {
        if (foreground && listening?.isActive == true) return
        // Woken by a push: a pooled connection may have died while the process was frozen or the phone slept, and a
        // request on it fails only after the ping times out; the push then ends in the fallback. A fresh one instead.
        transport.forgetConnections(now = true)
        syncing.withLock {
            val c = client ?: return
            absorb(c.savedEvents(), c)
            absorb(c.sync(0u), c)
            if (c === client) {
                mutable.update { it.copy(lastSync = System.currentTimeMillis()) }
            }
        }
        // As Signal's "you may have new messages": gone once the inbox came through.
        notifications.clear(FALLBACK)
        // Keep the process eligible to complete downloads started by the inbox pass.
        withTimeoutOrNull(15_000) {
            state.first { snapshot -> snapshot.media.none { !it.ready } }
        }
    }

    suspend fun pushFallback() = changes.withLock {
        if (!foreground) {
            crashes.countNotificationFallback("pushSyncFailed")
            try {
                notifications.show(FALLBACK, "$FALLBACK:${java.util.UUID.randomUUID()}", null, null, MessageKind.Yo)
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
        // Never added unchecked: without the hub's answer the name could be nobody's.
        // No answer: the banner says why (`Problems`), the name stays in the field for another try.
        val found = c.whois(name) ?: error(copy(ProductText.NO_SUCH_USER))
        if (c !== client) return
        val known = known(found.username ?: name, found.user)
        if (known != null) { showSent(known, copy(ProductText.ALREADY_ADDED)); return }
        addFriend(c, found.username ?: name, found.user)
    }
    private fun known(spelled: String, user: String?) = state.value.friends.firstOrNull { it.group == null && !it.gone &&
        ((user != null && it.user == user) || usernameSkeleton(it.name) == usernameSkeleton(spelled)) }
    private suspend fun addFriend(c: YoClient, spelled: String, user: String?) {
        changes.withLock {
            if (known(spelled, user) != null) return@withLock
            user?.let { removedUsers.remove(it) }
            val friend = Friend(name = spelled, color = nextColor(state.value.friends), user = user)
            if (friend.user != null) c.setFriend(friend.user, friend.name, friend.color.toUByte())
            mutable.update { it.copy(friends = it.friends + friend) }; listFriends(); persist()
        }
        store(c)
    }
    suspend fun invite(name: String) {
        mutable.update { it.copy(pendingInvite = name) }; changes.withLock { persist() }; consumeInvite()
    }
    /** An invite link's or scanned code's name: asked of the hub, then a dialog asks to add it and send a Yo (only a
     *  Yo, for one already on the list). Never added unverified: while the hub does not answer it stays pending for the
     *  next start or connection. A name not on Yo says so once and never comes back (where it failed the start:
     *  "Can't Open Your Account"). */
    private suspend fun consumeInvite() {
        val c = client ?: return
        val name = state.value.pendingInvite ?: return
        val taken = suspend { mutable.update { it.copy(pendingInvite = null) }; changes.withLock { persist() } }
        // What is wrong with it is said in the invite's dialog, with only OK (as iOS).
        fun problem(text: ProductText) = mutable.update { it.copy(invitation = Invitation(name.uppercase(), null, false, copy(text))) }
        if (usernameSkeleton(name) == usernameSkeleton(state.value.username ?: "")) { taken(); problem(ProductText.THATS_YOU); return }
        val found = try { c.whois(name) } catch (e: CancellationException) { throw e } catch (e: Exception) {
            if (e is YoException.Replaced) throw e
            // No answer: asked again later, and the banner says why the link did nothing yet.
            if (e is YoException.Network || e is YoException.Refused) { Problems.report(e) { command { consumeInvite() } }; return }
            taken(); mutable.update { it.copy(error = friendlyError(context, e)) }; return
        }
        if (c !== client || state.value.pendingInvite != name) return
        taken()
        if (found == null) { problem(ProductText.NO_SUCH_USER); return }
        val spelled = found.username ?: name
        val known = known(spelled, found.user)
        mutable.update { it.copy(invitation = Invitation(known?.name ?: spelled, found.user, known != null)) }
    }
    /** "Add & Yo" (or "Yo"): the friend added as `add` puts it, then one Yo as a tap on its bar sends (and lifts it). */
    suspend fun acceptInvite(invitation: Invitation) {
        mutable.update { it.copy(invitation = null) }
        val c = client ?: return
        if (!invitation.known) addFriend(c, invitation.name, invitation.user)
        known(invitation.name, invitation.user)?.let { send(it) }
    }
    fun dismissInvite() { mutable.update { it.copy(invitation = null) } }
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
                Lat.mark("send.begin")
                val events = c.send(sendTarget(friend.target), MessageKind.Yo)
                Lat.mark("send.end")
                // The hub has it: the bar says so now; what the send brought along and the bookkeeping follow.
                showSent(friend)
                Lat.mark("sent.shown"); Lat.frame("sent")
                // Taken by the core, not by the hub yet (offline after sealing): "Waiting to Send" until it went.
                if (c.waiting() > 0u) Problems.report(ProductProblem.WAITING, ::retryNow)
                absorb(events, c)
                recordStat(friend,true)
                changes.withLock { persist() }
                if (vault.read("first-yo") == null) {
                    vault.write("first-yo", byteArrayOf(1)); mutable.update { it.copy(firstYo = true) }
                }
                deliverLater(c)
            } catch (e: Exception) {
                val reason = when (e) {
                    is YoException.NotOnYo -> ProductText.NOT_ON_YO
                    is YoException.GroupEmpty -> ProductText.GROUP_EMPTY
                    is YoException.Unconfirmed -> ProductText.CONFIRM_NEW_KEY
                    is YoException.Blocked -> ProductText.BLOCKED
                    is YoException.Outdated -> ProductText.NEEDS_UPDATE
                    else -> null
                }
                showSent(friend, copy(reason ?: ProductText.NOT_SENT))
                if (e is YoException.Replaced) throw e
                if (e is YoException.GroupEmpty) { mutable.update { it.copy(emptyGroup = friend) }; return@withLock }
                // A reason of its own is the bar's clear end; any other Yo is kept for the bar's tap ("Not Sent · Tap
                // to Retry"), never dropped. The banner says why.
                if (reason == null) keepUnsent(friend.target, 1)
                Problems.report(Problems.problemOf(e).let { if (it == ProductProblem.FAILED) ProductProblem.NOT_SENT else it }, ::retryNow)
                deliverLater(c)
            }
        }
    }
    /** The banner tapped: its problem's own retry, else everything that waits goes again now, not after its backoff. */
    fun bannerTapped() { if (!Problems.retry()) retryNow() }
    /** What waits goes again now: the core's outbox, every bar's unsent Yos and media. */
    fun retryNow() {
        val c = client ?: return
        retrying?.cancel(); retrying = null
        deliverLater(c); online.trySend(Unit)
        val targets = synchronized(unsentYos) { unsentYos.keys.toList() }
        command { targets.forEach { target -> retryYos(target) }; media.retry() }
    }
    /** A tap on a bar that says "Not Sent · Tap to Retry": what waits for it goes now, no new Yo. */
    fun retryBar(friend: Friend) = command { retryYos(friend.target); media.retry(friend.target) }
    private suspend fun retryYos(target: String) {
        val c = client ?: return
        val friend = state.value.friends.firstOrNull { it.target == target } ?: return
        val count = synchronized(unsentYos) { unsentYos.remove(target) } ?: return
        saveUnsent(); markUnsent(target)
        repeat(count) { sendNow(friend, c) }
    }
    private fun keepUnsent(target: String, count: Int) {
        synchronized(unsentYos) { unsentYos[target] = (unsentYos[target] ?: 0) + count }
        saveUnsent(); markUnsent(target)
    }
    private fun saveUnsent() {
        val kept = synchronized(unsentYos) { JSONObject(unsentYos.toMap()).toString() }
        try { vault.write("unsent-yos", kept.toByteArray()) } catch (e: Exception) { android.util.Log.w("Yo", "unsent not saved: ${e::class.simpleName}") }
    }
    private fun loadUnsent() {
        val kept = vault.read("unsent-yos")?.let { JSONObject(String(it)) } ?: return
        synchronized(unsentYos) { kept.keys().forEach { unsentYos[it] = kept.getInt(it) } }
        kept.keys().forEach(::markUnsent)
    }
    /** The bar of `target` says "Not Sent · Tap to Retry" while a Yo or media for it waits after a failed try. */
    private fun markUnsent(target: String) {
        val friend = state.value.friends.firstOrNull { it.target == target } ?: return
        val on = synchronized(unsentYos) { (unsentYos[target] ?: 0) > 0 || target in mediaUnsent }
        present { unsent(it, friend.key, on) }
    }
    private fun deliverLater(c: YoClient) {
        if (retrying?.isActive == true) return
        retrying = scope.launch(Dispatchers.IO) {
            var pause = 1000L
            while (c === client && !state.value.update) {
                try {
                    if (c.waiting() == 0u) { Problems.resolve(ProductProblem.WAITING); break }
                    absorb(c.deliver(), c)
                } catch (e: CancellationException) { throw e } catch (_: Exception) { /* Tried again with backoff; the banner says "Waiting to Send". */ }
                delay(pause); pause = (pause * 2).coerceAtMost(30000)
            }
        }
    }
    /** A send's end on the bar: `sent` (Yo) or a failure's words from the core. */
    fun showSent(friend: Friend, failure: String? = null, kind: ListKind = ListKind.YO) {
        listFriends()
        present { time -> if (failure == null) sendFinished(time, friend.key, kind) else sendFailed(time, friend.key, failure) }
    }
    private fun recordStat(friend: Friend,sent: Boolean) {
        val time=System.currentTimeMillis()
        mutable.update { s -> s.copy(sent=s.sent+if(sent) 1 else 0,received=s.received+if(sent) 0 else 1,
            byFriend=s.byFriend+(friend.key to ((s.byFriend[friend.key] ?: 0)+1)),
            lastSent=if(sent) s.lastSent+(friend.key to time) else s.lastSent,
            lastReceived=if(sent) s.lastReceived else s.lastReceived+(friend.key to time)) }
    }
    private val notifications = YoNotifications(context)
    /** A Yo shown on a bar while Yo is in front: the list plays its haptic (one per frame at most, as iOS). */
    val arrivals = kotlinx.coroutines.flow.MutableSharedFlow<Unit>(extraBufferCapacity = 1,
        onBufferOverflow = kotlinx.coroutines.channels.BufferOverflow.DROP_OLDEST)
    private fun arrival(friend: Friend, user: String?, kind: MessageKind, token: String) {
        Lat.mark("arrival ${token.substringAfterLast('/')}"); Lat.frame("arrival")
        present { received(it, friend.key, when(kind) {
            is MessageKind.Photo -> ListKind.PHOTO
            is MessageKind.Video -> ListKind.VIDEO
            else -> ListKind.YO
        }, token, if(friend.group != null) user else null) }
        recordStat(friend,false)
        if (foreground) { chime(); arrivals.tryEmit(Unit) }
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
            friends = if (old == null) friends + row else friends.map { if (it.key == old.key) row else it }
        }
        mutable.update { it.copy(friends = friends, requestsOpen = c.requestsOpen()) }; listFriends(); persist()
    }
    suspend fun absorb(events: List<YoEvent>, source: YoClient? = client) {
        if (events.isEmpty() || source == null || source !== client) return
        val c = source
        val profiles = events.filterIsInstance<YoEvent.Profile>().associate { it.user to it.name }
        changes.withLock {
            if (source !== client) return
            for (event in events) when (event) {
                is YoEvent.Message -> removedUsers.remove(event.user)
                is YoEvent.NewChat -> removedUsers.remove(event.user)
                else -> Unit
            }
            // A Yo from someone already on the list shows at once; the list's refresh (core lists, the encrypted
            // save) comes after it, not before (it cost 30-60 ms of every arrival).
            if (events.any { it is YoEvent.Message || it is YoEvent.GroupMessage }) {
                val contacts = c.contacts().associateBy { it.user }
                for (event in events) arrive(event, profiles, contacts)
            }
        }
        Lat.mark("absorb.refresh")
        refresh()
        Lat.mark("absorb.refreshed")
        changes.withLock {
            if (c !== client) return
            val contacts = c.contacts().associateBy { it.user }
            for (event in events) {
                if(event is YoEvent.Profile) present { senderNamed(it,event.user,event.name) }
                if (event is YoEvent.IdentityChanged) {
                    val friend = state.value.friends.firstOrNull { it.user == event.user }
                    if (friend != null) mutable.update { s -> s.copy(friends = s.friends.map { if (it.key == friend.key) it.copy(keyChanged = true) else it },
                        keyChanged = KeyChange(friend, contacts[event.user]?.unconfirmed == true)) }
                }
                arrive(event, profiles, contacts)
            }
            persist(); c.acknowledge(events)
        }
        media.wake()
    }
    /** A message event's Yo on its bar (once: `seen`), its media expected, a notification in the background. */
    private suspend fun arrive(event: YoEvent, profiles: Map<String, String>, contacts: Map<String, YoContact>) {
        val user = when (event) { is YoEvent.Message -> event.user; is YoEvent.GroupMessage -> event.user; else -> return }
        val group = (event as? YoEvent.GroupMessage)?.group
        val friend = state.value.friends.firstOrNull { if (group != null) it.group == group else it.user == user } ?: return
        val at = when (event) { is YoEvent.Message -> event.sentAtMs; is YoEvent.GroupMessage -> event.sentAtMs; else -> return }
        val kind = when (event) { is YoEvent.Message -> event.kind; is YoEvent.GroupMessage -> event.kind; else -> return }
        if (friend.blocked) return
        val id = "${friend.target}/$user/$at"
        // First save the media descriptor; an unsuccessful save leaves the event in the Rust journal.
        media.expect(id, friend.target, user, kind, at.toLong())
        if (!seen.add(id)) return
        if (seen.size > 2000) seen.remove(seen.first())
        arrival(friend, user, kind, id)
        val contact = contacts[user]
        val name = profiles[user]?.takeIf { it.isNotBlank() }
            ?: contact?.name?.takeIf { it.isNotBlank() }
            ?: contact?.handles?.firstOrNull()
            ?: state.value.friends.firstOrNull { it.user == user }?.name?.takeUnless { it == "Yo User" }
        if (name == null) {
            crashes.countNotificationFallback(if (group == null) "unknownSender" else "unknownGroupSender")
        }
        try {
            if (!foreground) notifications.show(friend.target, id, name, friend.name.takeIf { group != null }, kind,
                friend.key, client?.userId())
        } catch (_: SecurityException) {
            // Notification permission may be revoked between checking and posting.
        }
    }
    fun click() {
        val audio = context.getSystemService(android.media.AudioManager::class.java)
        if(audio.ringerMode == android.media.AudioManager.RINGER_MODE_NORMAL)
            audio.playSoundEffect(android.media.AudioManager.FX_KEY_CLICK)
    }
    /** The Yo sound, loaded once: a MediaPlayer made per arrival prepared on the main thread (~30-40 ms on the A34),
     *  exactly while the bar flashed. SoundPool plays from memory, from any thread, at most the core's limit at once. */
    private val sounds by lazy {
        val pool = android.media.SoundPool.Builder().setMaxStreams(productSoundLimit().toInt().coerceAtLeast(1))
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION).build())
            .build()
        pool to pool.load(context, R.raw.yo, 1)
    }
    fun preloadSound() { scope.launch(Dispatchers.IO) { sounds } }
    private fun chime() {
        val (pool, yo) = sounds
        pool.play(yo, 1f, 1f, 1, 0, 1f)
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
        if (synchronized(unsentYos) { unsentYos.remove(friend.target) } != null) saveUnsent()
        friend.user?.let { client?.removeFriend(it) }
        changes.withLock { friend.user?.let { removedUsers.add(it) }; mutable.update { it.copy(friends = it.friends.filter { f -> f.key != friend.key }) }; listFriends(); persist() }
        client?.let(::store)
    }
    suspend fun block(friend: Friend, blocked: Boolean) { client?.setBlocked(friend.target, blocked); refresh(); client?.let(::store); media.wake() }
    suspend fun report(friend: Friend, reason: ReportReason) { if (TestLab.isRunning(context)) return; client?.report(friend.target, reason, InstallId.current(context)) }
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
    // Help goes by the install id alone, signed in or not.
    fun feedback(text: String) = if (TestLab.isRunning(context)) Unit else helpSend(transport, InstallId.current(context), utf8Limit(text.trim(), 2000), BuildConfig.VERSION_NAME)
    /** Null until this install has an id: no thread to read yet. */
    fun supportThread() = InstallId.existing(context)?.let { helpThread(transport, it) }
    fun supportRead(through: ULong) {
        InstallId.existing(context)?.let { helpRead(transport, it, through) }
        supportRevision.incrementAndGet()
        mutable.update { it.copy(supportUnread = false) }
    }
    private val supportRevision = java.util.concurrent.atomic.AtomicLong()
    /** Whether the Yo team answered since the chat was last read (iOS `refreshSupport`); offline keeps the last answer. */
    fun refreshSupport() = scope.launch(Dispatchers.IO) {
        val revision = supportRevision.get()
        val unread = try { (supportThread()?.unread ?: 0u) > 0u } catch (e: CancellationException) { throw e } catch (_: Exception) { return@launch }
        if (revision == supportRevision.get()) mutable.update { it.copy(supportUnread = unread) }
    }
    suspend fun roundTrip() = client?.roundTripProbe("PINGTEST", 10000u)
    val crashes = CrashReports(context, transport)

    suspend fun settings(left: Boolean = state.value.leftHanded, requests: Boolean = state.value.requestsOpen) {
        if (requests != state.value.requestsOpen) client?.setRequestsOpen(requests)
        changes.withLock { mutable.update { it.copy(leftHanded = left, requestsOpen = requests) }; persist() }
    }
    suspend fun undoSettings(friend: Friend, on: Boolean, seconds: Int) = changes.withLock {
        mutable.update { it.copy(friends = it.friends.map { f -> if (f.key == friend.key) f.copy(undo = on, undoSeconds = productUndoSeconds(seconds.toDouble()).toInt()) else f }) }; persist()
    }
    suspend fun security() = client?.securityStatus()
    /** Read from the contact and encryption pages' own coroutines, outside [scope]: a core error (a closed client)
     *  goes where [command]'s do and shows no details, never escapes into the screen. */
    suspend fun contact(friend: Friend): ContactDetails? {
        val c=client ?: return null
        return try {
            val contact=c.contacts().firstOrNull { it.user==friend.user }
            ContactDetails(contact,friend.user?.let { c.accountKeyOf(it) },
                friend.user?.takeIf { contact!=null }?.let { c.safetyNumber(it) },friend.user?.let { c.sessionsWith(it) }.orEmpty(),
                friend.user?.takeIf { contact!=null }?.let { c.safetyCode(it) })
        } catch (e: CancellationException) { throw e } catch (e: Exception) { failed(e); null }
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
        // The names iOS uses (its alternate icons), so the choice syncs between both.
        val aliases = mapOf<String?,String>(null to "Default", "Negative" to "Negative", "Original" to "Original", "OG2014" to "OG2014", "Lilac2015" to "Lilac2015")
        val selected = aliases[name] ?: return
        val manager = context.packageManager
        fun component(value: String) = android.content.ComponentName(context.packageName, "app.justyo.android.${value}Icon")
        manager.setComponentEnabledSetting(component(selected), android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_ENABLED, android.content.pm.PackageManager.DONT_KILL_APP)
        aliases.values.filter { it != selected }.forEach { value -> manager.setComponentEnabledSetting(component(value), android.content.pm.PackageManager.COMPONENT_ENABLED_STATE_DISABLED, android.content.pm.PackageManager.DONT_KILL_APP) }
        client?.setAppIcon(name)
        mutable.update { it.copy(appIcon = name) }
        changes.withLock { persist() }
        client?.let(::store)
    }
    /** Account Backup (iOS AccountSyncRow): when Yo's server last held all of it (ms), null while a change waits to go
     *  up; throws while there is no account or it can't be read. */
    fun backupSavedAt(): Long? = (client ?: error("signed out")).storageSavedAt()?.toLong()
    /** Backs the account up now; false if that failed. */
    fun backUpNow(): Boolean = runCatching { (client ?: error("signed out")).writeStorage() }.isSuccess
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
        synchronized(unsentYos) { unsentYos.clear(); mediaUnsent.clear() }; Problems.clear()
        if (registeredPush?.first == sessionId - 1) runCatching { old?.setPush(0u, "") }
        registeredPush = null; pushSession.value = null
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
    fun emptyGroupSeen() { mutable.update { it.copy(emptyGroup = null) } }
    /** An empty group deleted: left without a notice (nobody would see it), its bar gone. */
    suspend fun deleteEmptyGroup(friend: Friend) {
        emptyGroupSeen()
        val c = client ?: return
        friend.group?.let { absorb(c.leaveGroup(it), c) }
        refresh()
    }
    fun keyChangeSeen() { mutable.update { it.copy(keyChanged = null) } }
    companion object {
        /** The thread of "You may have received a Yo", shown when a push's inbox sync failed. */
        const val FALLBACK = "yo-fallback"
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
    // The core's own words (a save "Busy", an internal step) are for the error log, never the screen.
    is YoException -> { android.util.Log.w("Yo", "core error: ${error::class.simpleName}"); context.getString(R.string.generic_failure) }
    else -> error.message?.takeIf { it.isNotBlank() } ?: context.getString(R.string.generic_failure)
}
