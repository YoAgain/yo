package app.justyo.android

import com.google.zxing.*
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.qrcode.QRCodeWriter
import org.junit.Assert.*
import org.junit.Test

class QrPayloadTest {
    @Test fun binarySafetyPayloadSurvivesTheSameQrCodecAsTheCamera() {
        val original=byteArrayOf(0,1,2,3,31,32,127,128.toByte(),255.toByte())+"safety code".toByteArray()
        val matrix=QRCodeWriter().encode(original.toString(Charsets.ISO_8859_1),BarcodeFormat.QR_CODE,256,256,mapOf(EncodeHintType.CHARACTER_SET to "ISO-8859-1"))
        val pixels=IntArray(256*256) { if(matrix[it%256,it/256]) 0xff000000.toInt() else 0xffffffff.toInt() }
        val result=QRCodeReader().decode(BinaryBitmap(HybridBinarizer(RGBLuminanceSource(256,256,pixels))))
        assertArrayEquals(original,scannedQr(result).bytes)
    }
}
