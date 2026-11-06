package com.lycoris.maps.feature.account

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import java.io.IOException
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Visibility
import androidx.compose.material.icons.rounded.VisibilityOff
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lycoris.maps.core.data.AccountRepository
import com.lycoris.maps.core.data.SessionIdentity
import com.lycoris.maps.core.data.isRegistrableUsername
import com.lycoris.maps.core.designsystem.LycorisTextStyles
import com.lycoris.maps.core.designsystem.LycorisColors
import com.lycoris.maps.core.media.PhotoFailure
import com.lycoris.maps.core.media.PhotoImporter
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.User
import com.lycoris.maps.core.network.ApiClients
import com.lycoris.maps.core.network.ApiFailure
import com.lycoris.maps.feature.map.groupShape
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The host supplies its title, close button, scrolling and insets; this is natural-height content. */
@Composable
fun AccountPanel(
    accounts: AccountRepository,
    clients: ApiClients,
    language: Language,
    page: AccountPage,
    onPage: (AccountPage) -> Unit,
    onSignedIn: () -> Unit,
    onClose: () -> Unit,
    onMyPlaces: () -> Unit,
) {
    val account by accounts.state.collectAsStateWithLifecycle()
    var passwordReset by remember { mutableStateOf(false) }
    // Logout clears the identity before its suspension returns. Keep its navigation callback
    // outside the identity-keyed form scope, while still cancelling when this whole panel closes.
    val panelScope = rememberCoroutineScope()
    val logoutAction = remember(panelScope) { AccountAction(panelScope) }
    val currentOnClose by rememberUpdatedState(onClose)
    val effectivePage = if (page.requiresSession && account.user == null) AccountPage.LOGIN else page
    LaunchedEffect(page, account.initialized, account.epoch, account.user?.publicId, account.busy) {
        if (page.requiresSession && account.initialized && account.user == null && !account.busy) onPage(AccountPage.LOGIN)
    }
    key(effectivePage, if (effectivePage.requiresSession) account.user?.publicId else "authentication", if (effectivePage.requiresSession) account.epoch else 0L) {
        val scope = rememberCoroutineScope()
        val action = remember(scope) { AccountAction(scope) }
        val keyboard = LocalSoftwareKeyboardController.current
        val focus = LocalFocusManager.current
        val currentOnPage by rememberUpdatedState(onPage)
        val currentOnSignedIn by rememberUpdatedState(onSignedIn)
        val busy = action.busy || logoutAction.busy || account.busy
        val formIdentity = account.user?.let { SessionIdentity(clients.origin.toString(), it.publicId, account.epoch) }
        fun submit(block: suspend () -> Unit) {
            keyboard?.hide()
            focus.clearFocus()
            action.run(block)
        }
        Column(
            Modifier.fillMaxWidth().padding(start = 30.dp, end = 30.dp, top = 8.dp, bottom = 22.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            (action.failure ?: logoutAction.failure ?: account.failure?.takeIf { it is ApiFailure.SessionRequired })?.let { failure ->
                AccountNotice(accountFailureMessage(failure, language, effectivePage), language) { action.dismiss(); logoutAction.dismiss(); accounts.dismissFailure() }
            }
            action.photoFailure?.let {
                AccountNotice(language.text("无法读取这张图片，请选择另一张图片。", "This image could not be read. Choose another image."), language) { action.dismiss() }
            }
            if (passwordReset && effectivePage == AccountPage.LOGIN) {
                AccountNotice(language.text("密码已重置，请使用新密码登录。", "Password reset. Sign in with your new password."), language) { passwordReset = false }
            }
            when (effectivePage) {
                AccountPage.LOGIN -> LoginForm(language, busy,
                    onSubmit = { username, password -> submit {
                        passwordReset = false
                        val user = accounts.login(username, password)
                        if (accounts.identity()?.publicId == user.publicId) currentOnSignedIn()
                    } },
                    onRegister = { currentOnPage(AccountPage.REGISTER) },
                    onForgot = { currentOnPage(AccountPage.RESET) },
                )
                AccountPage.REGISTER -> RegisterForm(language, busy,
                    onSubmit = { username, nickname, email, password, code -> submit {
                        val user = accounts.register(username, nickname, email, password, code)
                        if (accounts.identity()?.publicId == user.publicId) currentOnSignedIn()
                    } },
                    onLogin = { currentOnPage(AccountPage.LOGIN) },
                    failure = action.failure,
                    onSendCode = { email -> accounts.sendEmailCode(email, false, if (language == Language.ZH) "zh" else "en"); Unit },
                )
                AccountPage.RESET -> RecoveryForm(language, busy, action.failure,
                    onSendCode = { email -> accounts.sendEmailCode(email, true, if (language == Language.ZH) "zh" else "en"); Unit },
                    onSubmit = { email, code, password -> submit {
                        accounts.resetPassword(email, code, password)
                        passwordReset = true
                        currentOnPage(AccountPage.LOGIN)
                    } },
                    onLogin = { currentOnPage(AccountPage.LOGIN) },
                )
                AccountPage.PROFILE -> account.user?.let { user ->
                    ProfileContent(user, account.epoch, accounts, clients, language, busy, action,
                        onEdit = { currentOnPage(AccountPage.EDIT_PROFILE) },
                        onPassword = { currentOnPage(AccountPage.PASSWORD) },
                        onMyPlaces = onMyPlaces,
                        onLogout = {
                            keyboard?.hide()
                            focus.clearFocus()
                            logoutAction.run { accounts.logout(); currentOnClose() }
                        },
                    )
                }
                AccountPage.EDIT_PROFILE -> account.user?.let { user ->
                    EditProfileForm(user, language, busy) { nickname, pronouns, signature -> submit {
                        val updated = accounts.updateProfile(nickname, pronouns, signature, expectedIdentity = formIdentity)
                        if (accounts.identity()?.publicId == updated.publicId) currentOnPage(AccountPage.PROFILE)
                    } }
                }
                AccountPage.PASSWORD -> PasswordForm(language, busy) { old, new -> submit {
                    accounts.changePassword(old, new, expectedIdentity = formIdentity)
                    currentOnPage(if (accounts.identity() == null) AccountPage.LOGIN else AccountPage.PROFILE)
                } }
            }
        }
    }
}

private class AccountAction(private val scope: CoroutineScope) {
    var busy by mutableStateOf(false)
        private set
    var failure by mutableStateOf<ApiFailure?>(null)
        private set
    var photoFailure by mutableStateOf<PhotoFailure?>(null)
        private set

    fun dismiss() { failure = null; photoFailure = null }
    fun run(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        dismiss()
        scope.launch {
            try {
                block()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: ApiFailure) {
                failure = error
            } catch (error: PhotoFailure) {
                photoFailure = error
            } catch (_: IOException) {
                photoFailure = PhotoFailure.Storage()
            } catch (_: SecurityException) {
                photoFailure = PhotoFailure.Unsupported()
            } finally {
                busy = false
            }
        }
    }
}

@Composable
internal fun LoginForm(language: Language, busy: Boolean, onSubmit: (String, String) -> Unit, onRegister: () -> Unit, onForgot: () -> Unit = {}) {
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    val ready = username.isNotBlank() && password.isNotEmpty() && !busy
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        AccountField(username, { username = it }, language.text("账号或邮箱", "Username or Email"), enabled = !busy, keyboardType = KeyboardType.Email)
        AccountField(password, { password = it }, language.text("密码", "Password"), enabled = !busy, password = true, language = language,
            imeAction = ImeAction.Done, onDone = { if (ready) onSubmit(username, password) })
        AccountButton(language.text("登录", "Log In"), busy, ready) { onSubmit(username, password) }
        TextButton(onClick = onForgot, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
            Text(language.text("忘记密码？", "Forgot password?"))
        }
        TextButton(onClick = onRegister, enabled = !busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
            Text(language.text("创建账号", "Create an Account"), textAlign = TextAlign.Center)
        }
    }
}

@Composable
internal fun RegisterForm(language: Language, busy: Boolean, onSubmit: (String, String, String, String, String) -> Unit, onLogin: () -> Unit,
    failure: ApiFailure? = null, onSendCode: suspend (String) -> Unit) {
    var username by remember { mutableStateOf("") }
    var nickname by remember { mutableStateOf("") }
    var email by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    LaunchedEffect(email) { code = "" }
    val usernameRejected = !isRegistrableUsername(username)
    val ready = username.isNotBlank() && !usernameRejected && email.isNotBlank() && password.isNotEmpty() && code.length == 6 && !busy
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        AccountField(username, { username = it }, language.text("账号", "Username"), enabled = !busy,
            isError = usernameRejected, supportingText = if (usernameRejected) language.usernameRule() else null)
        AccountField(nickname, { nickname = it }, language.text("昵称（可选）", "Display Name (optional)"), enabled = !busy, capitalization = KeyboardCapitalization.Words)
        AccountField(email, { email = it }, language.text("邮箱", "Email"), enabled = !busy, keyboardType = KeyboardType.Email)
        AccountField(password, { password = it }, language.text("密码", "Password"), enabled = !busy, password = true, language = language,
            imeAction = ImeAction.Next)
        val codeBlocked = EmailCodeField(email, code, { code = it }, language, busy, failure, onSendCode)
        AccountButton(language.text("注册", "Register"), busy, ready && !codeBlocked) { onSubmit(username, nickname, email, password, code) }
        TextButton(onClick = onLogin, enabled = !busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
            Text(language.text("已有账号？登录", "Already have an account? Log In"), textAlign = TextAlign.Center)
        }
    }
}

@Composable
private fun EmailCodeField(email: String, code: String, onCode: (String) -> Unit, language: Language,
    busy: Boolean, failure: ApiFailure?, send: suspend (String) -> Unit): Boolean {
    val scope = rememberCoroutineScope()
    val currentEmail by rememberUpdatedState(email)
    var sending by remember { mutableStateOf(false) }
    var retryAt by remember(email) { mutableLongStateOf(0L) }
    var lockedUntil by remember(email) { mutableLongStateOf(0L) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    var notice by remember(email) { mutableStateOf<String?>(null) }
    fun receiveFailure(error: ApiFailure) {
        notice = accountFailureMessage(error, language, AccountPage.RESET)
        if (error is ApiFailure.Http && error.status == 429) {
            val seconds = error.retryAfterSeconds ?: if (error.serviceCode == 42931) 3600 else 60
            retryAt = System.currentTimeMillis() + seconds.coerceIn(1, 86400) * 1000L
            if (error.serviceCode == 42931) lockedUntil = retryAt
        }
    }
    LaunchedEffect(failure) { if (failure != null) receiveFailure(failure) }
    LaunchedEffect(Unit) { while (true) { now = System.currentTimeMillis(); kotlinx.coroutines.delay(1000) } }
    val remaining = ((maxOf(retryAt, lockedUntil) - now + 999) / 1000).coerceAtLeast(0)
    AccountField(code, { onCode(it.filter { c -> c in '0'..'9' }.take(6)) }, language.text("验证码", "Verification Code"),
        enabled = !busy && !sending && lockedUntil <= now, keyboardType = KeyboardType.Number)
    OutlinedButton(onClick = {
        if (!sending) {
            sending = true; notice = null
            val recipient = email
            scope.launch {
                try {
                    send(recipient)
                    if (currentEmail != recipient) return@launch
                    onCode(""); retryAt = System.currentTimeMillis() + 60000
                    notice = language.text("验证码已发送，10 分钟内有效。", "A code has been sent. It expires in 10 minutes.")
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (error: ApiFailure) { if (currentEmail == recipient) receiveFailure(error) }
                finally { sending = false }
            }
        }
    }, enabled = !busy && !sending && remaining == 0L && email.contains('@'),
        modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp), shape = RoundedCornerShape(22.dp)) {
        Text(if (remaining > 0) language.text("${remaining} 秒后重发", "Resend in ${remaining}s")
            else if (sending) language.text("正在发送…", "Sending…") else language.text("发送验证码", "Send code"))
    }
    notice?.let { Text(it, style = MaterialTheme.typography.bodySmall, modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
    return sending || lockedUntil > now
}

@Composable
private fun RecoveryForm(language: Language, busy: Boolean, failure: ApiFailure?, onSendCode: suspend (String) -> Unit,
    onSubmit: (String, String, String) -> Unit, onLogin: () -> Unit) {
    var email by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var confirmation by remember { mutableStateOf("") }
    LaunchedEffect(email) { code = "" }
    AccountField(email, { email = it }, language.text("邮箱", "Email"), enabled = !busy, keyboardType = KeyboardType.Email)
    AccountField(password, { password = it }, language.text("新密码", "New Password"), enabled = !busy, password = true, language = language)
    AccountField(confirmation, { confirmation = it }, language.text("确认密码", "Confirm Password"), enabled = !busy, password = true, language = language)
    val codeBlocked = EmailCodeField(email, code, { code = it }, language, busy, failure, onSendCode)
    AccountButton(language.text("重置密码", "Reset Password"), busy,
        !busy && !codeBlocked && email.contains('@') && code.length == 6 && password.length >= 4 && password == confirmation) { onSubmit(email, code, password) }
    TextButton(onClick = onLogin, enabled = !busy, modifier = Modifier.fillMaxWidth()) { Text(language.text("返回登录", "Back to login")) }
}

@Composable
private fun ProfileContent(
    user: User,
    epoch: Long,
    accounts: AccountRepository,
    clients: ApiClients,
    language: Language,
    busy: Boolean,
    action: AccountAction,
    onEdit: () -> Unit,
    onPassword: () -> Unit,
    onMyPlaces: () -> Unit,
    onLogout: () -> Unit,
) {
    val context = LocalContext.current
    var pickerIdentity by remember { mutableStateOf<SessionIdentity?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        val expected = pickerIdentity
        pickerIdentity = null
        if (uri != null && expected != null) action.run {
            if (accounts.identity() != expected) throw ApiFailure.SessionChanged()
            val importer = withContext(Dispatchers.IO) { PhotoImporter(context.applicationContext) }
            val photo = importer.importPhoto(uri)
            try {
                val file = withContext(Dispatchers.IO) { importer.files.verify(photo) }
                accounts.uploadAvatar(file, photo.mimeType, expectedIdentity = expected)
            } finally {
                withContext(NonCancellable + Dispatchers.IO) { importer.files.delete(photo) }
            }
        }
    }
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        AccountAvatar(user, epoch, clients, language, Modifier.size(72.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(user.displayName, style = MaterialTheme.typography.titleLarge)
            user.email?.takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = LycorisColors.SecondaryText) }
        }
    }
    TextButton(
        onClick = {
            pickerIdentity = accounts.identity()
            launcher.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
        },
        enabled = !busy,
        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
    ) {
        if (busy) CircularProgressIndicator(Modifier.padding(end = 10.dp).size(18.dp), strokeWidth = 2.dp)
        Text(language.text("更改头像", "Change Avatar"), textAlign = TextAlign.Center)
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf(
            language.text("编辑个人资料", "Edit Profile") to onEdit,
            language.text("更改密码", "Change Password") to onPassword,
            language.text("我的点位", "My Places") to onMyPlaces,
        ).forEachIndexed { index, (label, callback) ->
            Row(
                Modifier.fillMaxWidth().heightIn(min = 66.dp).clip(groupShape(index, 3)).background(LycorisColors.Card)
                    .clickable(enabled = !busy, role = Role.Button, onClick = callback).padding(horizontal = 16.dp, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(label, Modifier.weight(1f), style = LycorisTextStyles.SettingsRow)
                Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, Modifier.padding(start = 4.dp).size(20.dp), tint = LycorisColors.Plum)
            }
        }
    }
    OutlinedButton(onClick = onLogout, enabled = !busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), shape = RoundedCornerShape(24.dp)) {
        Text(language.text("退出登录", "Log Out"), textAlign = TextAlign.Center)
    }
}

@Composable
private fun EditProfileForm(user: User, language: Language, busy: Boolean, onSave: (String, String, String) -> Unit) {
    var nickname by remember(user.publicId, user.nickname) { mutableStateOf(user.nickname.orEmpty()) }
    var pronouns by remember(user.publicId, user.pronouns) { mutableStateOf(user.pronouns.orEmpty()) }
    var signature by remember(user.publicId, user.signature) { mutableStateOf(user.signature.orEmpty()) }
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        AccountField(nickname, { nickname = it }, language.text("昵称", "Display Name"), enabled = !busy, capitalization = KeyboardCapitalization.Words)
        AccountField(pronouns, { pronouns = it }, language.text("代词", "Pronouns"), enabled = !busy)
        AccountField(signature, { signature = it }, language.text("个人简介", "Bio"), enabled = !busy, singleLine = false, capitalization = KeyboardCapitalization.Sentences,
            imeAction = ImeAction.Done, onDone = { if (!busy) onSave(nickname, pronouns, signature) })
        AccountButton(language.text("保存", "Save"), busy, !busy) { onSave(nickname, pronouns, signature) }
    }
}

@Composable
internal fun PasswordForm(language: Language, busy: Boolean, onSave: (String, String) -> Unit) {
    var old by remember { mutableStateOf("") }
    var new by remember { mutableStateOf("") }
    var confirmation by remember { mutableStateOf("") }
    val mismatch = confirmation.isNotEmpty() && new != confirmation
    val ready = old.isNotEmpty() && new.isNotEmpty() && new == confirmation && !busy
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        AccountField(old, { old = it }, language.text("当前密码", "Current Password"), enabled = !busy, password = true, language = language)
        AccountField(new, { new = it }, language.text("新密码", "New Password"), enabled = !busy, password = true, language = language)
        AccountField(confirmation, { confirmation = it }, language.text("确认新密码", "Confirm New Password"), enabled = !busy, password = true, language = language, isError = mismatch,
            imeAction = ImeAction.Done, onDone = { if (ready) onSave(old, new) })
        if (mismatch) Text(language.text("两次输入的密码不一致。", "The new passwords do not match."), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
        AccountButton(language.text("更改密码", "Change Password"), busy, ready) { onSave(old, new) }
    }
}

@Composable
private fun AccountField(
    value: String,
    onValue: (String) -> Unit,
    label: String,
    enabled: Boolean,
    password: Boolean = false,
    language: Language = Language.EN,
    keyboardType: KeyboardType = KeyboardType.Text,
    capitalization: KeyboardCapitalization = KeyboardCapitalization.None,
    imeAction: ImeAction = ImeAction.Next,
    singleLine: Boolean = true,
    isError: Boolean = false,
    /** Shown below the field and announced as it appears, so a disabled action has a stated reason. */
    supportingText: String? = null,
    onDone: () -> Unit = {},
) {
    var revealed by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    OutlinedTextField(
        value = value,
        onValueChange = onValue,
        modifier = Modifier.fillMaxWidth().heightIn(min = 60.dp),
        enabled = enabled,
        label = { Text(label) },
        supportingText = supportingText?.let { text ->
            { Text(text, Modifier.semantics { liveRegion = LiveRegionMode.Polite }) }
        },
        shape = RoundedCornerShape(16.dp),
        singleLine = singleLine,
        minLines = if (singleLine) 1 else 3,
        maxLines = if (singleLine) 1 else 5,
        isError = isError,
        visualTransformation = if (password && !revealed) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(
            capitalization = capitalization,
            autoCorrectEnabled = !password && keyboardType == KeyboardType.Text && capitalization != KeyboardCapitalization.None,
            keyboardType = if (password) KeyboardType.Password else keyboardType,
            imeAction = imeAction,
        ),
        keyboardActions = KeyboardActions(onNext = { focus.moveFocus(androidx.compose.ui.focus.FocusDirection.Next) }, onDone = { onDone() }),
        trailingIcon = if (password) ({
            IconButton(onClick = { revealed = !revealed }, enabled = enabled, modifier = Modifier.size(48.dp)) {
                Icon(if (revealed) Icons.Rounded.VisibilityOff else Icons.Rounded.Visibility,
                    if (revealed) language.text("隐藏密码", "Hide password") else language.text("显示密码", "Show password"))
            }
        }) else null,
    )
}

@Composable
private fun AccountButton(label: String, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    Button(onClick = onClick, enabled = enabled, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), shape = RoundedCornerShape(24.dp)) {
        if (busy) CircularProgressIndicator(Modifier.padding(end = 10.dp).size(18.dp), strokeWidth = 2.dp)
        Text(label, textAlign = TextAlign.Center)
    }
}

@Composable
private fun AccountNotice(message: String, language: Language, onDismiss: () -> Unit) {
    Surface(shape = RoundedCornerShape(16.dp), color = MaterialTheme.colorScheme.errorContainer) {
        Row(Modifier.fillMaxWidth().semantics { liveRegion = LiveRegionMode.Polite }, verticalAlignment = Alignment.CenterVertically) {
            Text(message, Modifier.weight(1f).padding(start = 16.dp, top = 12.dp, bottom = 12.dp), color = MaterialTheme.colorScheme.onErrorContainer, style = MaterialTheme.typography.bodyMedium)
            IconButton(onDismiss, Modifier.size(48.dp)) { Icon(Icons.Rounded.Close, language.text("关闭提示", "Dismiss message")) }
        }
    }
}
