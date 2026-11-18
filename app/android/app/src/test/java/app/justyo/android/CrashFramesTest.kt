package app.justyo.android

import org.junit.Assert.*
import org.junit.Test

class CrashFramesTest {
    private fun number(id: Int,value: Int)=byteArrayOf((id*8).toByte(),value.toByte())
    private fun varint(value: Int): ByteArray { var n=value;val out=java.io.ByteArrayOutputStream();while(n>=128) { out.write((n and 127) or 128);n=n ushr 7 };out.write(n);return out.toByteArray() }
    private fun bytes(id: Int,value: ByteArray)=varint(id*8+2)+varint(value.size)+value
    @Test fun tombstoneKeepsOnlyCrashedThreadCodeOffsets() {
        val frame=number(1,42)+bytes(6,"/private/account/libyo.so".toByteArray())+bytes(4,"secret message".toByteArray())
        val other=number(1,5)+bytes(6,"/private/personal-name.so".toByteArray())
        val thread=bytes(4,frame)+bytes(4,other)+bytes(2,"private-thread".toByteArray())
        val tombstone=number(6,7)+bytes(16,number(1,7)+bytes(2,thread))+bytes(14,"secret abort message".toByteArray())
        assertEquals("libyo.so +0x2a",CrashFrames.nativeFrames(tombstone))
    }
    @Test fun malformedAndOversizedTombstonesAreDiscarded() {
        assertEquals("",CrashFrames.nativeFrames(byteArrayOf(0x82.toByte())))
        assertEquals("",CrashFrames.nativeFrames(ByteArray(1024*1024+1)))
    }
    @Test fun managedCrashNeverContainsMessageOrFilePath() {
        val error=IllegalStateException("private message")
        error.stackTrace=arrayOf(StackTraceElement("app.justyo.android.Repository","send","/private/name.kt",42),StackTraceElement("private.account.Name","run","secret",1))
        assertEquals("app.justyo.android.Repository.send:42",CrashFrames.javaFrames(error))
    }
}
