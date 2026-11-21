package app.justyo.android

import android.content.Context
import android.provider.Settings

/** Google Play's pre-launch report robots (Firebase Test Lab) type sample text; nothing they write may reach the team. */
internal object TestLab {
    fun isRunning(context: Context): Boolean = isRunning(runCatching { Settings.System.getString(context.contentResolver, "firebase.test.lab") }.getOrNull())
    fun isRunning(setting: String?) = setting == "true"
}
