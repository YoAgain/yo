package app.justyo.android

import android.Manifest
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Log
import android.view.Surface
import androidx.camera.core.MirrorMode
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.video.FileOutputOptions
import androidx.camera.video.Quality
import androidx.camera.video.QualitySelector
import androidx.camera.video.Recorder
import androidx.camera.video.Recording
import androidx.camera.video.VideoCapture
import androidx.camera.video.VideoRecordEvent
import androidx.camera.view.PreviewView
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner
import kotlinx.coroutines.*
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.UUID
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.effect.Presentation
import androidx.media3.transformer.*

class YoCamera(private val context: Context) {
    private var provider: ProcessCameraProvider? = null
    private var camera: Camera? = null
    private var image: ImageCapture? = null
    private var video: VideoCapture<Recorder>? = null
    private var recording: Recording? = null
    private var binding = 0
    var front = false
        private set
    var zoom by mutableFloatStateOf(1f)
        private set
    fun bind(owner: LifecycleOwner, view: PreviewView, failed: (String) -> Unit) {
        val request = ++binding
        image = null
        video = null
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            if (request != binding) return@addListener
            try {
                val p=future.get(); provider=p; p.unbindAll()
                val preferred = if (front) CameraSelector.DEFAULT_FRONT_CAMERA else CameraSelector.DEFAULT_BACK_CAMERA
                if (!p.hasCamera(preferred)) front = !front
                val selector = if (front) CameraSelector.DEFAULT_FRONT_CAMERA else CameraSelector.DEFAULT_BACK_CAMERA
                val rotation = view.display?.rotation ?: Surface.ROTATION_0
                val preview = Preview.Builder().setTargetRotation(rotation).build().also { it.surfaceProvider = view.surfaceProvider }
                val photo = ImageCapture.Builder().setTargetRotation(rotation)
                    .setCaptureMode(ImageCapture.CAPTURE_MODE_MINIMIZE_LATENCY).build()
                val recorder = Recorder.Builder().setQualitySelector(QualitySelector.from(Quality.SD)).build()
                val movie = VideoCapture.Builder(recorder).setTargetRotation(rotation)
                    .setMirrorMode(MirrorMode.MIRROR_MODE_ON_FRONT_ONLY).build()
                camera=p.bindToLifecycle(owner,selector,preview,photo,movie)
                image=photo; video=movie; zoom=1f
            } catch (error: Exception) {
                Log.w("YoCamera", "Camera binding failed", error)
                failed(context.getString(R.string.camera_open_failed))
            }
        },ContextCompat.getMainExecutor(context))
    }
    fun flip() { front=!front }
    fun zoom() {
        val maximum=camera?.cameraInfo?.zoomState?.value?.maxZoomRatio ?: 1f
        zoom=if(zoom*2>maximum || zoom>=4f) 1f else zoom*2
        camera?.cameraControl?.setZoomRatio(zoom)
    }
    suspend fun photo(): Uri = withContext(Dispatchers.Main) {
        suspendCancellableCoroutine { done ->
            val file=File(context.cacheDir,"capture-${UUID.randomUUID()}.jpg")
            val capture=image
            if(capture==null) { done.resumeWithException(IllegalStateException(context.getString(R.string.camera_not_ready))); return@suspendCancellableCoroutine }
            val metadata = ImageCapture.Metadata().apply { isReversedHorizontal = front }
            capture.takePicture(ImageCapture.OutputFileOptions.Builder(file).setMetadata(metadata).build(),ContextCompat.getMainExecutor(context),object: ImageCapture.OnImageSavedCallback {
                override fun onImageSaved(result: ImageCapture.OutputFileResults) { if(done.isActive) done.resume(Uri.fromFile(file)) else file.delete() }
                override fun onError(exception: ImageCaptureException) { file.delete(); if(done.isActive) done.resumeWithException(exception) }
            })
        }
    }
    fun startVideo(done: (Uri?) -> Unit) {
        if(recording!=null) return
        val capture=video ?: return done(null)
        val file=File(context.cacheDir,"capture-${UUID.randomUUID()}.mp4")
        val options=FileOutputOptions.Builder(file).setDurationLimitMillis(VideoLimitMs).build()
        var pending=capture.output.prepareRecording(context,options)
        if(ContextCompat.checkSelfPermission(context,Manifest.permission.RECORD_AUDIO)==PackageManager.PERMISSION_GRANTED) pending=pending.withAudioEnabled()
        recording=pending.start(ContextCompat.getMainExecutor(context)) { event ->
            if(event is VideoRecordEvent.Finalize) {
                recording=null
                val ok=!event.hasError() || event.error==VideoRecordEvent.Finalize.ERROR_DURATION_LIMIT_REACHED
                done(if(ok) Uri.fromFile(file) else null)
                if(!ok) file.delete()
            }
        }
    }
    fun stopVideo() { recording?.stop() }
    fun close() {
        binding++
        recording?.close()
        provider?.unbindAll()
        camera = null
        image = null
        video = null
    }
}
internal const val VideoLimitMs = 10_000L

data class PreparedMedia(val bytes: ByteArray,val width: Int,val height: Int,val video: Boolean=false,val duration: Int=0)
object MediaFiles {
    suspend fun photo(context: Context,uri: Uri): PreparedMedia = withContext(Dispatchers.IO) {
        val source=ImageDecoder.createSource(context.contentResolver,uri)
        val bitmap=ImageDecoder.decodeBitmap(source) { decoder,info,_ ->
            val ratio=minOf(1.0,1600.0/maxOf(info.size.width,info.size.height))
            decoder.setTargetSize((info.size.width*ratio).toInt().coerceAtLeast(1),(info.size.height*ratio).toInt().coerceAtLeast(1))
            decoder.allocator=ImageDecoder.ALLOCATOR_SOFTWARE
        }
        val bytes=ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.JPEG,70,it) }.toByteArray()
        PreparedMedia(bytes,bitmap.width,bitmap.height).also { bitmap.recycle() }
    }
    @androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
    suspend fun video(context: Context,uri: Uri): PreparedMedia {
        val file=File(context.cacheDir,"prepared-${UUID.randomUUID()}.mp4")
        try {
            withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<Unit> { done ->
                    val item=MediaItem.Builder().setUri(uri).setClippingConfiguration(MediaItem.ClippingConfiguration.Builder().setEndPositionMs(VideoLimitMs).build()).build()
                    val edited=EditedMediaItem.Builder(item).setEffects(Effects(emptyList(),listOf(Presentation.createForHeight(540)))).build()
                    val transformer=Transformer.Builder(context).setVideoMimeType(MimeTypes.VIDEO_H264).setAudioMimeType(MimeTypes.AUDIO_AAC)
                        .addListener(object: Transformer.Listener {
                            override fun onCompleted(composition: Composition,exportResult: ExportResult) { if(done.isActive) done.resume(Unit) }
                            override fun onError(composition: Composition,exportResult: ExportResult,exportException: ExportException) { if(done.isActive) done.resumeWithException(exportException) }
                        }).build()
                    done.invokeOnCancellation { transformer.cancel() }
                    transformer.start(edited,file.absolutePath)
                }
            }
            return withContext(Dispatchers.IO) {
                require(file.length() in 1..9_000_000) { context.getString(R.string.video_too_large) }
                val retriever=android.media.MediaMetadataRetriever()
                try {
                    retriever.setDataSource(file.path)
                    PreparedMedia(file.readBytes(),retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)?.toInt() ?: 540,
                        retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toInt() ?: 960,true,
                        (retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_DURATION)?.toInt() ?: 10000).coerceAtMost(10000))
                } finally { retriever.release() }
            }
        } finally { withContext(NonCancellable + Dispatchers.IO) { file.delete() } }
    }
    suspend fun keep(context: Context,media: PreparedMedia) = withContext(Dispatchers.IO) {
        // Android 10+ adds our own media without asking for broad library access.
        if(Build.VERSION.SDK_INT<29) return@withContext
        val values=ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME,"Yo-${UUID.randomUUID()}.${if(media.video) "mp4" else "jpg"}")
            put(MediaStore.MediaColumns.MIME_TYPE,if(media.video) "video/mp4" else "image/jpeg")
            put(MediaStore.MediaColumns.RELATIVE_PATH,if(media.video) "Movies/Yo" else "Pictures/Yo")
            put(MediaStore.MediaColumns.IS_PENDING,1)
        }
        val collection=if(media.video) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        val uri=context.contentResolver.insert(collection,values) ?: return@withContext
        try {
            context.contentResolver.openOutputStream(uri)?.use { it.write(media.bytes) } ?: error(context.getString(R.string.ui_couldn_t_save_media))
            values.clear(); values.put(MediaStore.MediaColumns.IS_PENDING,0); context.contentResolver.update(uri,values,null,null)
        } catch(e: Exception) { context.contentResolver.delete(uri,null,null); throw e }
    }
}
