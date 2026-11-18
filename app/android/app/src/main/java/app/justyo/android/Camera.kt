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
import android.view.OrientationEventListener
import androidx.camera.core.MirrorMode
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.heifwriter.HeifWriter
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
import androidx.media3.common.util.Clock
import androidx.media3.exoplayer.mediacodec.MediaCodecSelector
import androidx.media3.effect.Presentation
import androidx.media3.common.audio.ChannelMixingAudioProcessor
import androidx.media3.common.audio.ChannelMixingMatrix
import androidx.media3.common.audio.SonicAudioProcessor
import android.media.MediaMetadataRetriever
import android.os.Handler
import android.os.Looper
import kotlin.math.roundToInt
import androidx.media3.transformer.*

class YoCamera(private val context: Context) {
    private var provider: ProcessCameraProvider? = null
    private var camera: Camera? = null
    private var image: ImageCapture? = null
    private var video: VideoCapture<Recorder>? = null
    private var recording: Recording? = null
    private var binding = 0
    private var previewView: PreviewView? = null
    private var physicalRotation: Int? = null
    private val orientation = object : OrientationEventListener(context) {
        override fun onOrientationChanged(degrees: Int) {
            if (degrees == ORIENTATION_UNKNOWN) return
            physicalRotation = when (degrees) {
                in 45..134 -> Surface.ROTATION_270
                in 135..224 -> Surface.ROTATION_180
                in 225..314 -> Surface.ROTATION_90
                else -> Surface.ROTATION_0
            }
        }
    }
    var front = false
        private set
    var zoom by mutableFloatStateOf(1f)
        private set
    fun bind(owner: LifecycleOwner, view: PreviewView, failed: (String) -> Unit) {
        val request = ++binding
        previewView = view
        orientation.enable()
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
                val recorder = Recorder.Builder()
                    .setQualitySelector(QualitySelector.from(Quality.HD, androidx.camera.video.FallbackStrategy.lowerQualityOrHigherThan(Quality.HD)))
                    .setTargetAudioChannelCount(2)
                    .setTargetAudioEncodingBitRate(128_000)
                    .build()
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
            capture.targetRotation = physicalRotation ?: previewView?.display?.rotation ?: capture.targetRotation
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
        capture.targetRotation = physicalRotation ?: previewView?.display?.rotation ?: capture.targetRotation
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
        orientation.disable()
        physicalRotation = null
        previewView = null
        camera = null
        image = null
        video = null
    }
}
internal const val VideoLimitMs = 10_000L
/** A shared or picked video (spec 5): up to 10 s; Android has no system trimmer, so a longer one is refused. */
internal const val SharedVideoLimitMs = VideoLimitMs
/** What a video may be at most: under a blob's 10 MiB (MAX_BLOB_LEN, its tag included) with room to spare. */
internal const val LargestVideo = 9_000_000L
private const val VideoBitrate = 2_500_000

data class PreparedMedia(val bytes: ByteArray,val width: Int,val height: Int,val video: Boolean=false,val duration: Int=0, val heic: Boolean=false)
object MediaFiles {
    suspend fun photo(context: Context,uri: Uri): PreparedMedia = withContext(Dispatchers.IO) {
        val source=ImageDecoder.createSource(context.contentResolver,uri)
        val bitmap=ImageDecoder.decodeBitmap(source) { decoder,info,_ ->
            val ratio=minOf(1.0,2048.0/maxOf(info.size.width,info.size.height))
            decoder.setTargetSize((info.size.width*ratio).toInt().coerceAtLeast(1),(info.size.height*ratio).toInt().coerceAtLeast(1))
            decoder.allocator=ImageDecoder.ALLOCATOR_SOFTWARE
        }
        val file = File(context.cacheDir, "prepared-${UUID.randomUUID()}.heic")
        try {
            val heic = try {
                HeifWriter.Builder(file.path, bitmap.width, bitmap.height, HeifWriter.INPUT_MODE_BITMAP)
                    .setQuality(75).build().use { writer ->
                        writer.start()
                        writer.addBitmap(bitmap)
                        writer.stop(5_000)
                    }
                file.takeIf { it.length() > 0 }?.readBytes()
            } catch (e: Exception) {
                Log.d("YoMedia", "HEIC encoder unavailable", e)
                null
            }
            val bytes = heic ?: ByteArrayOutputStream().also {
                check(bitmap.compress(Bitmap.CompressFormat.JPEG, 75, it))
            }.toByteArray()
            PreparedMedia(bytes, bitmap.width, bitmap.height, heic = heic != null)
        } finally {
            file.delete()
            bitmap.recycle()
        }
    }
    @androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
    suspend fun video(context: Context,uri: Uri,fromCamera: Boolean = false): PreparedMedia {
        val file=File(context.cacheDir,"prepared-${UUID.randomUUID()}.mp4")
        try {
            val (width, height, lengthMs) = withContext(Dispatchers.IO) {
                val retriever = MediaMetadataRetriever()
                try {
                    retriever.setDataSource(context, uri)
                    val lengthMs = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLongOrNull() ?: 0L
                    val maxInputMs = SharedVideoLimitMs + if (fromCamera) 500L else 0L
                    require(lengthMs <= maxInputMs) { context.getString(R.string.video_too_long) }
                    val w = checkNotNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)).toInt()
                    val h = checkNotNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)).toInt()
                    val rotation = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION)?.toInt()
                    val scale = minOf(1.0, 1280.0 / maxOf(w, h), 720.0 / minOf(w, h))
                    fun fit(side: Int) = ((side * scale / 2).roundToInt() * 2).coerceAtLeast(2)
                    if (rotation == 90 || rotation == 270) Triple(fit(h), fit(w), lengthMs) else Triple(fit(w), fit(h), lengthMs)
                } finally { retriever.release() }
            }
            withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<Unit> { done ->
                    val item=MediaItem.Builder().setUri(uri).setClippingConfiguration(MediaItem.ClippingConfiguration.Builder().setEndPositionMs(SharedVideoLimitMs).build()).build()
                    val stereo = ChannelMixingAudioProcessor().apply {
                        for (channels in 1..6) {
                            putChannelMixingMatrix(ChannelMixingMatrix.createForConstantPower(channels, 2))
                        }
                    }
                    val sampleRate = SonicAudioProcessor().apply { setOutputSampleRateHz(48_000) }
                    val edited = EditedMediaItem.Builder(item).setEffects(Effects(
                        listOf(stereo, sampleRate),
                        listOf(Presentation.createForWidthAndHeight(width, height, Presentation.LAYOUT_SCALE_TO_FIT)),
                    )).build()
                    val encoder = DefaultEncoderFactory.Builder(context)
                        .setRequestedVideoEncoderSettings(VideoEncoderSettings.Builder().setBitrate(VideoBitrate).setiFrameIntervalSeconds(2f).build())
                        .setRequestedAudioEncoderSettings(AudioEncoderSettings.Builder().setBitrate(128_000).build())
                        .setEnableFormatFallback(false)
                        .build()
                    val builder = Transformer.Builder(context).setVideoMimeType(MimeTypes.VIDEO_H265)
                        .setAudioMimeType(MimeTypes.AUDIO_AAC)
                    if (Build.HARDWARE == "ranchu" || Build.HARDWARE == "goldfish") {
                        // The virtual hardware decoder can fail after accepting the input format.
                        val decoder = DefaultDecoderFactory.Builder(context)
                            .setMediaCodecSelector(MediaCodecSelector.PREFER_SOFTWARE).build()
                        builder.setAssetLoaderFactory(ExoPlayerAssetLoader.Factory(context, decoder, Clock.DEFAULT))
                    }
                    val transformer = builder.setPortraitEncodingEnabled(true)
                        .setUsePlatformDiagnostics(false)
                        .setEncoderFactory(encoder)
                        .addListener(object: Transformer.Listener {
                            override fun onCompleted(composition: Composition,exportResult: ExportResult) { if(done.isActive) done.resume(Unit) }
                            override fun onError(composition: Composition,exportResult: ExportResult,exportException: ExportException) { if(done.isActive) done.resumeWithException(exportException) }
                        }).build()
                    done.invokeOnCancellation { Handler(Looper.getMainLooper()).post { transformer.cancel(); file.delete() } }
                    transformer.start(edited,file.absolutePath)
                }
            }
            return withContext(Dispatchers.IO) {
                require(file.length() in 1..LargestVideo) { context.getString(R.string.video_too_large) }
                val retriever=android.media.MediaMetadataRetriever()
                try {
                    retriever.setDataSource(file.path)
                    PreparedMedia(file.readBytes(),retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)?.toInt() ?: width,
                        retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)?.toInt() ?: height,true,
                        (retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_DURATION)?.toInt() ?: lengthMs.toInt()).coerceAtMost(SharedVideoLimitMs.toInt()))
                } finally { retriever.release() }
            }
        } finally { withContext(NonCancellable + Dispatchers.IO) { file.delete() } }
    }
    suspend fun keep(context: Context,media: PreparedMedia) = withContext(Dispatchers.IO) {
        // Android 10+ adds our own media without asking for broad library access.
        if(Build.VERSION.SDK_INT<29) return@withContext
        val values=ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME,"Yo-${UUID.randomUUID()}.${if(media.video) "mp4" else if(media.heic) "heic" else "jpg"}")
            put(MediaStore.MediaColumns.MIME_TYPE,if(media.video) "video/mp4" else if(media.heic) "image/heic" else "image/jpeg")
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
