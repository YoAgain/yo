package app.justyo.android

import android.app.ActivityManager
import android.content.Context
import android.provider.Settings

/** Google Play's pre-launch report robots (Firebase Test Lab) type sample text; nothing they write may reach the team.
 *  Test Lab's documented flag, plus the platform's own "a monkey/test harness drives this app" (Play's crawlers do not always set the flag). */
internal object TestLab {
    @Suppress("DEPRECATION")
    fun isRunning(context: Context): Boolean =
        isRunning(runCatching { Settings.System.getString(context.contentResolver, "firebase.test.lab") }.getOrNull()) ||
            runCatching { ActivityManager.isUserAMonkey() }.getOrDefault(false) ||
            runCatching { Settings.Global.getString(context.contentResolver, "firebase.test.lab") }.getOrNull() == "true"
    fun isRunning(setting: String?) = setting == "true"
}
