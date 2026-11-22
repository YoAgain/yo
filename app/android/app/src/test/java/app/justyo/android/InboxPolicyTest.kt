package app.justyo.android

import androidx.work.ExistingWorkPolicy
import androidx.work.WorkInfo
import org.junit.Assert.assertEquals
import org.junit.Test

class InboxPolicyTest {
    @Test fun aWaitingPassIsReplacedSoTheNewOneRunsNow() {
        // A retry waiting for WorkManager's wall-clock backoff: after the clock was set back, for hours.
        assertEquals(ExistingWorkPolicy.REPLACE, YoWork.inboxPolicy(listOf(WorkInfo.State.ENQUEUED)))
        assertEquals(ExistingWorkPolicy.REPLACE, YoWork.inboxPolicy(listOf(WorkInfo.State.ENQUEUED, WorkInfo.State.BLOCKED)))
        assertEquals(ExistingWorkPolicy.REPLACE, YoWork.inboxPolicy(emptyList()))
    }
    @Test fun aRunningPassIsFollowedNotCut() {
        assertEquals(ExistingWorkPolicy.APPEND_OR_REPLACE, YoWork.inboxPolicy(listOf(WorkInfo.State.RUNNING, WorkInfo.State.BLOCKED)))
        assertEquals(ExistingWorkPolicy.APPEND_OR_REPLACE, YoWork.inboxPolicy(null))
    }
}
