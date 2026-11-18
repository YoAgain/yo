package app.justyo.android

import android.graphics.Bitmap
import android.graphics.ImageDecoder
import java.nio.ByteBuffer
import android.media.MediaDataSource
import android.media.MediaMetadataRetriever
import java.io.ByteArrayOutputStream

/** Bounded decode from memory; video extraction never writes a plaintext temporary file. */
internal fun mediaThumbnail(bytes: ByteArray,video: Boolean): ByteArray? {
    val bitmap=if(video) runCatching {
        val reader=MediaMetadataRetriever()
        try {
            reader.setDataSource(object: MediaDataSource() {
                override fun getSize()=bytes.size.toLong()
                override fun close()=Unit
                override fun readAt(position: Long,buffer: ByteArray,offset: Int,size: Int): Int {
                    if(position<0 || position>=bytes.size) return -1
                    val count=minOf(size,bytes.size-position.toInt());bytes.copyInto(buffer,offset,position.toInt(),position.toInt()+count);return count
                }
            })
            reader.getScaledFrameAtTime(0,MediaMetadataRetriever.OPTION_CLOSEST_SYNC,256,256)
        } finally { reader.release() }
    }.getOrNull() else runCatching {
        ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, info, _ ->
            val scale = minOf(1.0, 256.0 / maxOf(info.size.width, info.size.height))
            decoder.setTargetSize((info.size.width * scale).toInt().coerceAtLeast(1),
                (info.size.height * scale).toInt().coerceAtLeast(1))
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
    }.getOrNull()
    return bitmap?.let { ByteArrayOutputStream().also { out -> it.compress(Bitmap.CompressFormat.JPEG,80,out);it.recycle() }.toByteArray() }
}
