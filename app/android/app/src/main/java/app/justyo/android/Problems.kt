package app.justyo.android

import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import uniffi.yo.*

/**
 * Why a user action did not happen, said in the one banner floating over the list (the "Offline" line; offline wins, as the
 * core decides). Every action that needs the hub reports its failure here, never nowhere. The same problem shows once
 * (again: its clock starts over). One that goes by itself (no connection, the hub busy, waiting to send) stays until
 * the hub answers again or what waits went; any other hides after 6 s. A tap on the banner tries the newest one again.
 */
object Problems {
    private class Entry(val retry: (() -> Unit)?, var expiry: Job?)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    /** By problem: a core [ProductProblem], or this phone's own words ([say]). */
    private val active = LinkedHashMap<Any, Entry>()
    private val shown = MutableStateFlow<String?>(null)
    /** The banner's words now: the newest problem's. */
    val text = shown.asStateFlow()

    fun report(problem: ProductProblem, retry: (() -> Unit)? = null) = add(problem, standing(problem), retry)

    /** Stays until the hub answers: what goes by itself (the core's `passing`), and a wrong clock, which nothing
     *  corrects until the person sets it; the first request that goes through then (a 2xx) ends it. */
    internal fun standing(problem: ProductProblem) = productProblemPassing(problem) || problem == ProductProblem.WRONG_CLOCK

    /** What this phone itself lacks for an action (no screen lock), in its own words; it hides after 6 s as well. */
    fun say(text: String, retry: (() -> Unit)? = null) = add(text, false, retry)

    private fun add(problem: Any, passing: Boolean, retry: (() -> Unit)?) = synchronized(active) {
        active.remove(problem)?.expiry?.cancel()
        val entry = Entry(retry, null)
        active[problem] = entry
        if (!passing) entry.expiry = scope.launch { delay(6000); resolve(problem, entry) }
        publish()
    }

    /** A failure as the person reads it: a core error by the core's own mapping, no answer as "No Connection". */
    fun report(error: Throwable, retry: (() -> Unit)? = null) {
        if (error is CancellationException) return
        report(problemOf(error), retry)
    }

    fun problemOf(error: Throwable): ProductProblem = when (error) {
        is YoException -> productProblemOf(error)
        is java.io.IOException -> ProductProblem.NO_CONNECTION
        else -> ProductProblem.FAILED
    }

    fun resolve(problem: ProductProblem) = synchronized(active) {
        val entry = active.remove(problem) ?: return@synchronized
        entry.expiry?.cancel(); publish()
    }

    private fun resolve(problem: Any, entry: Entry) = synchronized(active) {
        if (active[problem] === entry) { active.remove(problem); publish() }
    }

    /** The hub answered (`status`): no connection and an unreachable hub are over; a 2xx also ends "Too Many Tries"
     *  and a wrong clock (a request signed and dated by it went through). */
    fun answered(status: Int) {
        if (status >= 500 || status == 429) return
        resolve(ProductProblem.NO_CONNECTION); resolve(ProductProblem.SERVER_UNREACHABLE)
        if (status in 200..299) { resolve(ProductProblem.TOO_MANY_TRIES); resolve(ProductProblem.WRONG_CLOCK) }
    }

    /** The banner tapped: the newest problem's own retry; false when it has none. */
    fun retry(): Boolean {
        val retry = synchronized(active) { active.values.lastOrNull()?.retry } ?: return false
        retry(); return true
    }

    /** Signed out: nothing of the account before is left to tell. */
    fun clear() = synchronized(active) {
        active.values.forEach { it.expiry?.cancel() }; active.clear(); publish()
    }

    private fun publish() {
        shown.value = active.keys.lastOrNull()?.let { if (it is ProductProblem) productProblemText(it, Words) else it as String }
    }
}
