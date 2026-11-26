package app.justyo.android

import android.content.Context
import uniffi.yo.YoClient
import uniffi.yo.YoNameField

internal object Attestation {
    fun forSignUp(context: Context, field: YoNameField) { field.deviceDone(0u, null) }
    fun check(context: Context, client: YoClient) {}
}
