package app.justyo.android

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.*
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/** Camera analysis owns one bounded frame at a time; never logs or saves the scanned data. */
@Composable internal fun QrScanner(onClose: ()->Unit,onCode: (String)->Unit) {
    val context=LocalContext.current
    val owner=LocalLifecycleOwner.current
    var granted by remember { mutableStateOf(ContextCompat.checkSelfPermission(context,Manifest.permission.CAMERA)==PackageManager.PERMISSION_GRANTED) }
    var failed by remember { mutableStateOf(false) }
    val request=rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted=it }
    LaunchedEffect(Unit) { if(!granted) request.launch(Manifest.permission.CAMERA) }
    val deliver by rememberUpdatedState(onCode)
    val preview=remember { PreviewView(context) }
    DisposableEffect(granted,owner) {
        val executor=Executors.newSingleThreadExecutor()
        val disposed=AtomicBoolean(false)
        val completed=AtomicBoolean(false)
        val display=Preview.Builder().build().also { it.surfaceProvider=preview.surfaceProvider }
        val analysis=ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
        val main=ContextCompat.getMainExecutor(context)
        val future=ProcessCameraProvider.getInstance(context)
        var provider: ProcessCameraProvider?=null
        if(granted) {
            var lastScan=0L
            analysis.setAnalyzer(executor) { image ->
                try {
                    if(disposed.get() || completed.get()) return@setAnalyzer
                    val now=android.os.SystemClock.elapsedRealtime()
                    if(now-lastScan<100) return@setAnalyzer
                    lastScan=now
                    val plane=image.planes.first();val width=image.width;val height=image.height
                    val data=ByteArray(width*height);val buffer=plane.buffer
                    for(row in 0 until height) for(col in 0 until width) data[row*width+col]=buffer.get(row*plane.rowStride+col*plane.pixelStride)
                    val source=PlanarYUVLuminanceSource(data,width,height,0,0,width,height,false)
                    val result=MultiFormatReader().decode(BinaryBitmap(HybridBinarizer(source)),mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE)))
                    val code=result.text
                    if(completed.compareAndSet(false,true)) main.execute { if(!disposed.get()) deliver(code) }

                } catch(_: NotFoundException) { /* Next frame. */ }
                catch(_: Exception) { /* A malformed QR never stops camera analysis. */ }
                finally { image.close() }
            }
            future.addListener({
                if(!disposed.get()) try { provider=future.get();provider?.bindToLifecycle(owner,CameraSelector.DEFAULT_BACK_CAMERA,display,analysis) }
                catch(_: Exception) { failed=true }
            },main)
        }
        onDispose { disposed.set(true);analysis.clearAnalyzer();provider?.unbind(display,analysis);executor.shutdown() }
    }
    Dialog(onDismissRequest=onClose,properties=DialogProperties(usePlatformDefaultWidth=false)) {
        Surface(Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize().safeDrawingPadding()) {
                Row(Modifier.fillMaxWidth().padding(16.dp),horizontalArrangement=Arrangement.SpaceBetween) {
                    Text(stringResource(R.string.scan_qr),style=MaterialTheme.typography.titleLarge)
                    TextButton(onClick=onClose){Text(stringResource(R.string.ui_cancel))}
                }
                if(granted && !failed) AndroidView(factory={preview},modifier=Modifier.fillMaxWidth().weight(1f))
                else Text(stringResource(if(failed) R.string.camera_unavailable else R.string.camera_permission_needed),modifier=Modifier.padding(24.dp))
            }
        }
    }
}
