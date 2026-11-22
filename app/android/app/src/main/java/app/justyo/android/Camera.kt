package app.justyo.android

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.os.Build
import android.util.Log
import android.view.Surface
import android.view.OrientationEventListener
import androidx.camera.core.MirrorMode
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.SessionConfig
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.yo.productRecordZoom
import uniffi.yo.productCameraZoomChoices
import uniffi.yo.productCameraZoomNext
import androidx.core.content.ContextCompat
import androidx.core.net.toUri
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
    /** The invite names of the Yo codes in view (other QR codes left out), on the main thread; never while filming. */
    var codes: ((List<String>) -> Unit)? = null
    private var codeReader: java.util.concurrent.ExecutorService? = null
    @Volatile private var filming = false
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
    /** The zoom button's choices (the core's rule: 1, the ultra wide when the camera goes below 1, 2), as iOS. */
    var zoomChoices by mutableStateOf(listOf(1.0))
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
                camera?.cameraInfo?.zoomState?.removeObservers(owner)
                // QR codes are read only where the camera runs all four streams as they are: a phone that would
                // have to share one stream for them, or cannot at all, reads none (photos and videos come first).
                val codes = codeAnalysis()
                val all = runCatching { p.getCameraInfo(selector).isSessionConfigSupported(SessionConfig(listOf(preview,photo,movie,codes))) }
                camera = if (all.getOrDefault(false)) try { p.bindToLifecycle(owner,selector,preview,photo,movie,codes) }
                    catch (_: IllegalArgumentException) { p.unbindAll(); p.bindToLifecycle(owner,selector,preview,photo,movie) }
                else p.bindToLifecycle(owner,selector,preview,photo,movie)
                image=photo; video=movie; zoom=1f; heldZoomStart=null
                // The range is known once the camera runs: the choices follow it.
                camera?.cameraInfo?.zoomState?.observe(owner) { range ->
                    val least = range.minZoomRatio.toDouble()
                    val choices = productCameraZoomChoices(least, range.maxZoomRatio.toDouble(), listOfNotNull(least.takeIf { it < 0.95 }))
                    if (choices != zoomChoices) zoomChoices = choices
                }
            } catch (error: Exception) {
                Log.w("YoCamera", "Camera binding failed", error)
                failed(context.getString(R.string.camera_open_failed))
            }
        },ContextCompat.getMainExecutor(context))
    }
    /** Small frames, four a second, read off the main thread; the analyzer drops each at once while filming. */
    private fun codeAnalysis(): ImageAnalysis {
        codeReader?.shutdown()
        val reader = java.util.concurrent.Executors.newSingleThreadExecutor().also { codeReader = it }
        val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .setResolutionSelector(ResolutionSelector.Builder().setResolutionStrategy(ResolutionStrategy(
                android.util.Size(640, 480), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER)).build())
            .build()
        var last = 0L
        analysis.setAnalyzer(reader) { image ->
            try {
                val now = android.os.SystemClock.elapsedRealtime()
                if (filming || now - last < 250) return@setAnalyzer
                last = now
                val name = inviteName(readQr(image).text.trim().toUri()) ?: return@setAnalyzer
                main.post { if (!filming) codes?.invoke(listOf(name)) }
            } catch (_: com.google.zxing.NotFoundException) { /* No code in this frame. */ }
            catch (_: Exception) { /* A malformed code never stops the camera. */ }
            finally { image.close() }
        }
        return analysis
    }
    /** The zoom a filming hold started at; the finger drawn out of the bar zooms from there, lifted it goes back. */
    private var heldZoomStart: Float? = null
    private var heldZoomTicked = false
    /** Filming, the finger at [y] drawn out of its bar ([barTop] to [barBottom], down or up, on a screen [height]
     *  high, all dp) zooms in as far as `productRecordZoom` says, back on the bar out again; true once per hold, as
     *  it starts (for the tick). */
    fun recordZoom(y: Float, barTop: Float, barBottom: Float, height: Float): Boolean {
        val active = camera ?: return false
        val start = heldZoomStart ?: zoom.also { heldZoomStart = it }
        val most = active.cameraInfo.zoomState.value?.maxZoomRatio ?: 1f
        val next = productRecordZoom(y.toDouble(), barTop.toDouble(), barBottom.toDouble(), height.toDouble(),
            start.toDouble(), most.toDouble()).toFloat()
        if (kotlin.math.abs(next - zoom) > 0.001f) {
            zoom = next
            active.cameraControl.setZoomRatio(next)
        }
        val began = !heldZoomTicked && next > start + 0.01f
        if (began) heldZoomTicked = true
        return began
    }
    fun recordZoomEnd() {
        heldZoomStart?.let { start ->
            if (zoom != start) { zoom = start; camera?.cameraControl?.setZoomRatio(start) }
        }
        heldZoomStart = null
        heldZoomTicked = false
    }
    fun flip() { front=!front }
    fun zoom() {
        zoom = productCameraZoomNext(zoomChoices, zoom.toDouble()).toFloat()
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
    private val main = Handler(Looper.getMainLooper())
    private var waiting: Pair<Runnable, (Uri?) -> Unit>? = null
    fun startVideo(done: (Uri?) -> Unit) {
        if(recording!=null || waiting!=null) return
        val capture=video
        if(capture==null) {
            // Held right after the camera opened: the recorder is still being bound (over a second on a first open).
            // Wait for it while the finger stays, instead of "Couldn't record video" at once.
            val until=android.os.SystemClock.uptimeMillis()+3000
            val retry=object : Runnable {
                override fun run() {
                    when {
                        video!=null -> { waiting=null; startVideo(done) }
                        android.os.SystemClock.uptimeMillis()<until -> main.postDelayed(this,50)
                        else -> { waiting=null; done(null) }
                    }
                }
            }
            waiting=retry to done
            main.post(retry)
            return
        }
        capture.targetRotation = physicalRotation ?: previewView?.display?.rotation ?: capture.targetRotation
        val file=File(context.cacheDir,"capture-${UUID.randomUUID()}.mp4")
        val options=FileOutputOptions.Builder(file).setDurationLimitMillis(VideoLimitMs).build()
        var pending=capture.output.prepareRecording(context,options)
        if(ContextCompat.checkSelfPermission(context,Manifest.permission.RECORD_AUDIO)==PackageManager.PERMISSION_GRANTED) pending=pending.withAudioEnabled()
        recording=pending.start(ContextCompat.getMainExecutor(context)) { event ->
            if(event is VideoRecordEvent.Finalize) {
                recording=null
                filming=false
                val ok=!event.hasError() || event.error==VideoRecordEvent.Finalize.ERROR_DURATION_LIMIT_REACHED
                done(if(ok) Uri.fromFile(file) else null)
                if(!ok) file.delete()
            }
        }
        filming=true
    }
    fun stopVideo() { cancelWaiting(); recording?.stop() }
    private fun cancelWaiting() { waiting?.let { (retry, done) -> waiting=null; main.removeCallbacks(retry); done(null) } }
    fun close() {
        binding++
        cancelWaiting()
        recording?.close()
        provider?.unbindAll()
        codeReader?.shutdown()
        codeReader = null
        filming = false
        orientation.disable()
        physicalRotation = null
        previewView = null
        camera = null
        image = null
        video = null
    }
}
/** Media limits and encoding, the core's (`yo_product`). A longer shared video is refused: Android has no trimmer. */
internal val VideoLimitMs = uniffi.yo.productVideoLimitMs().toLong()
internal val LargestVideo = uniffi.yo.productVideoLimitBytes().toLong()
private val PhotoLongSide = uniffi.yo.productPhotoLongSide().toDouble()
private val Encoding = uniffi.yo.productEncoding()

data class PreparedMedia(val bytes: ByteArray,val width: Int,val height: Int,val video: Boolean=false,val duration: Int=0, val heic: Boolean=false)
object MediaFiles {
    suspend fun photo(context: Context,uri: Uri): PreparedMedia = withContext(Dispatchers.IO) {
        val source=ImageDecoder.createSource(context.contentResolver,uri)
        val bitmap=ImageDecoder.decodeBitmap(source) { decoder,info,_ ->
            val ratio=minOf(1.0,PhotoLongSide/maxOf(info.size.width,info.size.height))
            decoder.setTargetSize((info.size.width*ratio).toInt().coerceAtLeast(1),(info.size.height*ratio).toInt().coerceAtLeast(1))
            decoder.allocator=ImageDecoder.ALLOCATOR_SOFTWARE
        }
        val file = File(context.cacheDir, "prepared-${UUID.randomUUID()}.heic")
        try {
            val heic = try {
                HeifWriter.Builder(file.path, bitmap.width, bitmap.height, HeifWriter.INPUT_MODE_BITMAP)
                    .setQuality((Encoding.photoQuality * 100).roundToInt()).build().use { writer ->
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
                check(bitmap.compress(Bitmap.CompressFormat.JPEG, (Encoding.photoQuality * 100).roundToInt(), it))
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
                    val maxInputMs = VideoLimitMs + if (fromCamera) 500L else 0L
                    require(lengthMs <= maxInputMs) { copy(uniffi.yo.ProductText.VIDEO_TOO_LONG) + " · " + uniffi.yo.productVideoLimitLabel() }
                    val w = checkNotNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)).toInt()
                    val h = checkNotNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)).toInt()
                    val rotation = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION)?.toInt()
                    val scale = minOf(1.0, Encoding.videoLongSide.toDouble() / maxOf(w, h), Encoding.videoShortSide.toDouble() / minOf(w, h))
                    fun fit(side: Int) = ((side * scale / 2).roundToInt() * 2).coerceAtLeast(2)
                    if (rotation == 90 || rotation == 270) Triple(fit(h), fit(w), lengthMs) else Triple(fit(w), fit(h), lengthMs)
                } finally { retriever.release() }
            }
            withContext(Dispatchers.Main) {
                suspendCancellableCoroutine<Unit> { done ->
                    val item=MediaItem.Builder().setUri(uri).setClippingConfiguration(MediaItem.ClippingConfiguration.Builder().setEndPositionMs(VideoLimitMs).build()).build()
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
                        .setRequestedVideoEncoderSettings(VideoEncoderSettings.Builder().setBitrate(Encoding.videoBitRate.toInt()).setiFrameIntervalSeconds(2f).build())
                        .setRequestedAudioEncoderSettings(AudioEncoderSettings.Builder().setBitrate(Encoding.audioBitRate.toInt()).build())
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
                            // Its words are the codec's ("Codec exception: CodecInfo{…}"), never the screen's.
                            override fun onError(composition: Composition,exportResult: ExportResult,exportException: ExportException) {
                                if(done.isActive) done.resumeWithException(IllegalStateException(context.getString(R.string.video_not_prepared),exportException))
                            }
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
                        (retriever.extractMetadata(android.media.MediaMetadataRetriever.METADATA_KEY_DURATION)?.toInt() ?: lengthMs.toInt()).coerceAtMost(VideoLimitMs.toInt()))
                } finally { retriever.release() }
            }
        } finally { withContext(NonCancellable + Dispatchers.IO) { file.delete() } }
    }
}
