package com.lycoris.maps.app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.hardware.display.DisplayManager
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.Build
import android.provider.Settings
import android.view.Surface
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lycoris.maps.core.device.HeadingController
import com.lycoris.maps.core.device.HeadingState
import com.lycoris.maps.core.device.collectLocatedHeading
import com.lycoris.maps.core.device.LocationController
import com.lycoris.maps.core.device.LocationPermission
import com.lycoris.maps.core.device.LocationState
import com.lycoris.maps.core.device.LocationStatus
import com.lycoris.maps.core.device.VoiceController
import com.lycoris.maps.core.device.VoiceError
import com.lycoris.maps.core.device.VoiceState
import com.lycoris.maps.core.device.VoiceStatus
import com.lycoris.maps.core.device.QA_LOCAL_NETWORK_PERMISSION
import com.lycoris.maps.core.device.QA_LOCAL_NETWORK_REQUESTED
import com.lycoris.maps.core.device.StartupPermission
import com.lycoris.maps.core.device.StartupPermissionPolicy
import com.lycoris.maps.core.map.NativeMapState
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.BuildConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

data class DeviceActions(
    val location: LocationState,
    val heading: HeadingState,
    val voice: VoiceState,
    val onLocate: () -> Unit,
    val onVoice: () -> Unit,
    val cancelVoice: () -> Unit,
)

private enum class DeviceSettings { LOCATION_PERMISSION, LOCATION_SERVICE, MICROPHONE_PERMISSION }

/** Foreground platform services and explicit permission continuations for the root map. */
@Composable
fun rememberDeviceActions(
    map: NativeMapState,
    language: Language,
    onTranscript: (String) -> Unit,
    onMessage: (String) -> Unit,
    allowInitialCenter: Boolean = true,
    onBackgroundMessage: (String) -> Unit = onMessage,
    onLocalNetworkGranted: () -> Unit = {},
): DeviceActions {
    val context = LocalContext.current
    val activity = remember(context) { context.deviceActivity() }
    val view = LocalView.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val preferences = remember(context) { context.applicationContext.getSharedPreferences("device-permissions", Context.MODE_PRIVATE) }
    val locationController = remember(context) { LocationController(context.applicationContext) }
    val headingController = remember(context) { HeadingController(context.applicationContext) }
    val voiceController = remember(context) { VoiceController(context.applicationContext) }
    val location by locationController.state.collectAsStateWithLifecycle()
    val heading by headingController.state.collectAsStateWithLifecycle()
    val voice by voiceController.state.collectAsStateWithLifecycle()
    val currentLanguage by rememberUpdatedState(language)
    val message by rememberUpdatedState(onMessage)
    val backgroundMessage by rememberUpdatedState(onBackgroundMessage)
    val transcript by rememberUpdatedState(onTranscript)
    val localNetworkGranted by rememberUpdatedState(onLocalNetworkGranted)
    var foreground by remember { mutableStateOf(lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) }
    var initialLocationCentered by rememberSaveable { mutableStateOf(false) }
    var locateRequested by rememberSaveable { mutableStateOf(false) }
    var voiceRequested by remember { mutableStateOf(false) }
    // ActivityResultRegistry restores pending dialogs across configuration changes. Preserve
    // the queue's matching state so a second permission cannot launch over the first one.
    var microphoneRequestInFlight by rememberSaveable { mutableStateOf(false) }
    var locationRequestInFlight by rememberSaveable { mutableStateOf(false) }
    var localNetworkRequestInFlight by rememberSaveable { mutableStateOf(false) }
    var settings by remember { mutableStateOf<DeviceSettings?>(null) }
    val requiresLocalNetwork = StartupPermissionPolicy.requiresQaLocalNetwork(BuildConfig.TEST_ENVIRONMENT, Build.VERSION.SDK_INT)

    val locationLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        locationRequestInFlight = false
        locationController.refreshPermission()
        if (locationController.currentPermission() != LocationPermission.NONE) {
            if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) locationController.start()
        } else {
            val report = if (locateRequested) message else backgroundMessage
            locateRequested = false
            report(currentLanguage.deviceText("未授权定位。你仍然可以移动地图查找点位。", "Location is not allowed. You can still move the map to find places."))
        }
    }
    val microphoneLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        microphoneRequestInFlight = false
        // The system may deliver this before ON_RESUME; the pending user intent is consumed there.
        voiceRequested = granted && voiceRequested
        if (!granted) {
            voiceRequested = false
            message(currentLanguage.deviceText("未授权麦克风。你可以继续使用文字搜索。", "Microphone access is not allowed. Text search is still available."))
        }
    }
    val localNetworkLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        localNetworkRequestInFlight = false
        if (requiresLocalNetwork) {
            if (granted) localNetworkGranted()
            else backgroundMessage(currentLanguage.deviceText(
                "未允许本地网络访问，暂时无法连接测试服务。可在应用的系统权限设置中开启。",
                "Local network access was not allowed. Enable it in app permissions to connect to test services.",
            ))
        }
    }

    DisposableEffect(lifecycle, locationController, headingController, voiceController, view) {
        fun updateRotation() { headingController.setDisplayRotation(view.display?.rotation ?: Surface.ROTATION_0) }
        fun sync() {
            val started = lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
            val resumed = lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)
            foreground = resumed
            if (started) {
                updateRotation()
                locationController.start()
            } else {
                locationController.stop()
                headingController.stop()
            }
            voiceController.setForeground(resumed)
        }
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP && !microphoneRequestInFlight) voiceRequested = false
            sync()
        }
        val displayManager = context.getSystemService(DisplayManager::class.java)
        val displayObserver = object : DisplayManager.DisplayListener {
            override fun onDisplayAdded(displayId: Int) = Unit
            override fun onDisplayRemoved(displayId: Int) = Unit
            override fun onDisplayChanged(displayId: Int) {
                if (view.display?.displayId == displayId) updateRotation()
            }
        }
        lifecycle.addObserver(observer)
        displayManager?.registerDisplayListener(displayObserver, Handler(Looper.getMainLooper()))
        sync()
        onDispose {
            lifecycle.removeObserver(observer)
            displayManager?.unregisterDisplayListener(displayObserver)
            locationController.stop()
            headingController.stop()
            voiceController.destroy()
        }
    }

    LaunchedEffect(foreground, locationRequestInFlight, microphoneRequestInFlight, localNetworkRequestInFlight, settings) {
        val hasLocation = locationController.currentPermission() != LocationPermission.NONE
        val hasLocalNetwork = requiresLocalNetwork && ContextCompat.checkSelfPermission(context, QA_LOCAL_NETWORK_PERMISSION) == PackageManager.PERMISSION_GRANTED
        if (foreground) {
            // Pre-granted installs also count as handled: a later system revocation must not
            // cause automatic permission loops on every resume.
            if (hasLocation) preferences.edit().putBoolean("location-requested", true).apply()
            if (hasLocalNetwork) preferences.edit().putBoolean(QA_LOCAL_NETWORK_REQUESTED, true).apply()
        }
        when (StartupPermissionPolicy.next(
            foreground = foreground,
            permissionInFlight = locationRequestInFlight || microphoneRequestInFlight || localNetworkRequestInFlight,
            otherDialogOpen = settings != null,
            hasLocation = hasLocation,
            locationRequested = preferences.getBoolean("location-requested", false),
            requiresQaLocalNetwork = requiresLocalNetwork,
            hasQaLocalNetwork = hasLocalNetwork,
            qaLocalNetworkRequested = preferences.getBoolean(QA_LOCAL_NETWORK_REQUESTED, false),
        )) {
            StartupPermission.LOCATION -> {
                preferences.edit().putBoolean("location-requested", true).apply()
                locationRequestInFlight = true
                locationLauncher.launch(arrayOf(Manifest.permission.ACCESS_COARSE_LOCATION, Manifest.permission.ACCESS_FINE_LOCATION))
            }
            StartupPermission.QA_LOCAL_NETWORK -> {
                preferences.edit().putBoolean(QA_LOCAL_NETWORK_REQUESTED, true).apply()
                localNetworkRequestInFlight = true
                localNetworkLauncher.launch(QA_LOCAL_NETWORK_PERMISSION)
            }
            null -> Unit
        }
    }
    LaunchedEffect(foreground, voiceRequested, microphoneRequestInFlight) {
        if (foreground && voiceRequested && !microphoneRequestInFlight) {
            voiceRequested = false
            if (voiceController.hasRecordingPermission()) voiceController.start(currentLanguage.speechTag())
            else message(currentLanguage.deviceText("麦克风暂不可用，请检查系统权限与麦克风开关。", "The microphone is unavailable. Check system permissions and the microphone switch."))
        }
    }
    LaunchedEffect(lifecycle, locationController, headingController) {
        withContext(Dispatchers.Main.immediate) {
            lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
                // Observe location continuously: granting permission or receiving the first fix
                // must enable direction without another Activity start. Fix expiry also emits a
                // new location state, which stops sensors when there is no dot to attach it to.
                collectLocatedHeading(
                    locations = locationController.state,
                    nowNanos = android.os.SystemClock::elapsedRealtimeNanos,
                    updateLocation = headingController::updateLocation,
                    startHeading = headingController::start,
                    stopHeading = headingController::stop,
                )
            }
        }
    }
    LaunchedEffect(location.fix, map.ready, locateRequested, foreground, allowInitialCenter) {
        val fix = location.fix
        if (foreground && map.ready && fix != null && (locateRequested || (allowInitialCenter && !initialLocationCentered))) {
            map.moveTo(fix.latitude, fix.longitude, maxOf(map.camera.zoom, 15.0))
            initialLocationCentered = true
            locateRequested = false
        }
    }
    LaunchedEffect(location.status) {
        when (location.status) {
            LocationStatus.TIMED_OUT -> {
                val report = if (locateRequested) message else backgroundMessage
                locateRequested = false
                report(currentLanguage.deviceText("定位超时，请重试或移动地图。", "Location timed out. Retry or move the map."))
            }
            LocationStatus.DISABLED -> (if (locateRequested) message else backgroundMessage)(currentLanguage.deviceText("设备定位已关闭。请开启定位，或继续移动地图。", "Device location is turned off. Enable it or continue moving the map."))
            LocationStatus.UNAVAILABLE -> {
                val report = if (locateRequested) message else backgroundMessage
                locateRequested = false
                report(currentLanguage.deviceText("暂时无法获取位置，请重试或移动地图。", "Location is currently unavailable. Retry or move the map."))
            }
            else -> Unit
        }
    }
    LaunchedEffect(voice.status, voice.transcript, voice.error) {
        when (voice.status) {
            VoiceStatus.RESULT -> {
                val result = voice.transcript
                voiceController.consumeResult()
                if (result.isNotBlank()) transcript(result)
            }
            VoiceStatus.UNAVAILABLE -> {
                message(currentLanguage.deviceText("此设备暂不支持离线语音输入，请使用文字搜索。", "On-device speech input is unavailable on this device. Use text search."))
                voiceController.consumeResult()
            }
            VoiceStatus.PERMISSION_REQUIRED -> {
                message(currentLanguage.deviceText("麦克风权限不可用，请检查权限或使用文字搜索。", "Microphone access is unavailable. Check permissions or use text search."))
                voiceController.consumeResult()
            }
            VoiceStatus.ERROR -> {
                message(voiceFailureMessage(voice.error, currentLanguage))
                voiceController.consumeResult()
            }
            else -> Unit
        }
    }

    settings?.let { target ->
        fun dismissSettings() {
            settings = null
            if (target != DeviceSettings.MICROPHONE_PERMISSION) locateRequested = false
        }
        AlertDialog(
            onDismissRequest = ::dismissSettings,
            title = { Text(language.deviceText("权限与设置", "Permissions and Settings")) },
            text = { Text(when (target) {
                DeviceSettings.LOCATION_PERMISSION -> language.deviceText("在系统设置中允许 Lycoris 使用位置，便可定位并查找附近点位。", "Allow location access for Lycoris in system settings to locate yourself and find nearby places.")
                DeviceSettings.LOCATION_SERVICE -> language.deviceText("开启设备定位后，Lycoris 才能获取当前位置。", "Turn on device location so Lycoris can find your current position.")
                DeviceSettings.MICROPHONE_PERMISSION -> language.deviceText("在系统设置中允许麦克风后，再点一次语音搜索即可开始。", "Allow microphone access in system settings, then tap voice search again to start.")
            }) },
            confirmButton = { TextButton(onClick = {
                settings = null
                val intent = if (target == DeviceSettings.LOCATION_SERVICE) Intent(Settings.ACTION_LOCATION_SOURCE_SETTINGS)
                else Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
                if (activity == null) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                runCatching { context.startActivity(intent) }.onFailure {
                    message(currentLanguage.deviceText("无法打开系统设置，请手动打开。", "System settings could not be opened. Open them manually."))
                }
            }) { Text(language.deviceText("打开设置", "Open Settings")) } },
            dismissButton = { TextButton(onClick = ::dismissSettings) { Text(language.deviceText("取消", "Cancel")) } },
        )
    }

    return DeviceActions(location, heading, voice,
        onLocate = locate@{
            if (locationRequestInFlight || microphoneRequestInFlight || localNetworkRequestInFlight || settings != null) return@locate
            locateRequested = true
            when {
                locationController.currentPermission() == LocationPermission.NONE -> {
                    val canRequest = !preferences.getBoolean("location-requested", false) || activity?.let {
                        ActivityCompat.shouldShowRequestPermissionRationale(it, Manifest.permission.ACCESS_COARSE_LOCATION) ||
                            ActivityCompat.shouldShowRequestPermissionRationale(it, Manifest.permission.ACCESS_FINE_LOCATION)
                    } == true
                    if (canRequest && !locationRequestInFlight) {
                        preferences.edit().putBoolean("location-requested", true).apply()
                        locationRequestInFlight = true
                        locationLauncher.launch(arrayOf(Manifest.permission.ACCESS_COARSE_LOCATION, Manifest.permission.ACCESS_FINE_LOCATION))
                    } else if (!locationRequestInFlight) settings = DeviceSettings.LOCATION_PERMISSION
                }
                locationController.state.value.status == LocationStatus.DISABLED -> settings = DeviceSettings.LOCATION_SERVICE
                else -> locationController.start()
            }
        },
        onVoice = voice@{
            if (locationRequestInFlight || microphoneRequestInFlight || localNetworkRequestInFlight || settings != null) return@voice
            if (voiceController.state.value.isActive) {
                voiceRequested = false
                voiceController.cancel()
            } else if (!voiceController.isOnDeviceAvailable()) {
                voiceController.start(currentLanguage.speechTag())
            } else if (voiceController.hasRecordingPermission()) {
                voiceController.start(currentLanguage.speechTag())
            } else if (!microphoneRequestInFlight) {
                val canRequest = !preferences.getBoolean("microphone-requested", false) || activity?.let {
                    ActivityCompat.shouldShowRequestPermissionRationale(it, Manifest.permission.RECORD_AUDIO)
                } == true
                if (canRequest) {
                    preferences.edit().putBoolean("microphone-requested", true).apply()
                    voiceRequested = true
                    microphoneRequestInFlight = true
                    microphoneLauncher.launch(Manifest.permission.RECORD_AUDIO)
                } else settings = DeviceSettings.MICROPHONE_PERMISSION
            }
        },
        cancelVoice = { voiceRequested = false; voiceController.cancel() },
    )
}

private fun Context.deviceActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.takeIf { it !== this }?.deviceActivity()
    else -> null
}
private fun Language.deviceText(chinese: String, english: String): String = if (this == Language.ZH) chinese else english
private fun Language.speechTag(): String = if (this == Language.ZH) "zh-CN" else "en-US"

internal fun voiceFailureMessage(error: VoiceError?, language: Language): String = when (error) {
    VoiceError.NO_SPEECH, VoiceError.NO_MATCH -> language.deviceText("没有识别到语音，请再试一次或输入文字。", "No speech was recognized. Try again or type your search.")
    VoiceError.LANGUAGE_UNAVAILABLE -> language.deviceText("设备尚未提供此语言的离线语音识别，请使用文字搜索。", "On-device recognition for this language is unavailable. Use text search.")
    VoiceError.TIMED_OUT -> language.deviceText("语音输入超时，请重试。", "Speech input timed out. Please try again.")
    VoiceError.PERMISSION -> language.deviceText("麦克风权限不可用，请检查系统设置。", "Microphone access is unavailable. Check system settings.")
    else -> language.deviceText("暂时无法使用语音输入，请重试或输入文字。", "Speech input is unavailable right now. Retry or type your search.")
}
