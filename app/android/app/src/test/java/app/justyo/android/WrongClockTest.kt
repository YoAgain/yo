package app.justyo.android

import java.security.cert.CertPathValidatorException
import java.security.cert.CertificateException
import java.security.cert.CertificateExpiredException
import java.security.cert.CertificateNotYetValidException
import javax.net.ssl.SSLHandshakeException
import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.*

/** A phone whose clock is off says so ("Check Your Phone's Date & Time"), not "Offline", and keeps saying it. */
class WrongClockTest {
    private fun handshake(cause: Throwable) = SSLHandshakeException("Chain validation failed").apply {
        initCause(CertificateException(cause))
    }

    @Test fun certificateDatesAreTheClock() {
        assertTrue(refusedForDates(handshake(CertPathValidatorException("timestamp check failed", CertificateExpiredException()))))
        assertTrue(refusedForDates(handshake(CertificateNotYetValidException())))
        assertTrue(refusedForDates(handshake(CertPathValidatorException("timestamp check failed", null, null, -1,
            CertPathValidatorException.BasicReason.NOT_YET_VALID))))
        assertFalse(refusedForDates(handshake(CertPathValidatorException("Trust anchor for certification path not found."))))
        assertFalse(refusedForDates(java.net.UnknownHostException("dev.justyo.app")))
    }

    @Test fun aWrongClockStaysUntilTheHubAnswers() {
        assertTrue(Problems.standing(ProductProblem.WRONG_CLOCK))
        assertTrue(Problems.standing(ProductProblem.NO_CONNECTION))
        assertFalse(Problems.standing(ProductProblem.FAILED))
    }
}
