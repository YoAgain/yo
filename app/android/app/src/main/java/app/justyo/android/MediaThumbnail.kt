package app.justyo.android

import android.graphics.Bitmap
import android.graphics.BitmapFactory
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
    }.getOrNull() else {
        val bounds=BitmapFactory.Options().apply { inJustDecodeBounds=true }
        BitmapFactory.decodeByteArray(bytes,0,bytes.size,bounds)
        var sample=1
        while(maxOf(bounds.outWidth,bounds.outHeight)/sample>256) sample*=2
        BitmapFactory.decodeByteArray(bytes,0,bytes.size,BitmapFactory.Options().apply { inSampleSize=sample })
    }
    return bitmap?.let { ByteArrayOutputStream().also { out -> it.compress(Bitmap.CompressFormat.JPEG,80,out);it.recycle() }.toByteArray() }
}
