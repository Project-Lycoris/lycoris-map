package com.lycoris.maps.feature.account

import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.network.ApiFailure

enum class AccountPage {
    LOGIN, REGISTER, RESET, PROFILE, EDIT_PROFILE, PASSWORD;

    fun title(language: Language): String = when (this) {
        LOGIN -> language.text("登录", "Log In")
        RESET -> language.text("重置密码", "Reset Password")
        REGISTER -> language.text("注册", "Register")
        PROFILE -> language.text("个人主页", "Profile")
        EDIT_PROFILE -> language.text("编辑个人资料", "Edit Profile")
        PASSWORD -> language.text("更改密码", "Change Password")
    }

    val requiresSession: Boolean get() = this in setOf(PROFILE, EDIT_PROFILE, PASSWORD)
}

internal fun Language.text(chinese: String, english: String): String = if (this == Language.ZH) chinese else english

internal fun Language.usernameRule(): String = text("用户名不能包含 @。", "Usernames can't contain @.")

internal fun accountFailureMessage(failure: ApiFailure, language: Language, page: AccountPage): String = when (failure) {
    is ApiFailure.Http -> when {
        failure.serviceCode == 4005 -> language.usernameRule()
        failure.serviceCode == 40021 -> language.text("验证码错误或已过期。", "Verification code is invalid or expired.")
        failure.serviceCode == 40022 -> language.text("请输入有效的邮箱地址。", "Enter a valid email address.")
        failure.serviceCode == 42931 -> language.text("验证码错误达到 5 次，已冷却 1 小时。", "Too many incorrect codes. Try again in one hour.")
        failure.serviceCode == 42932 -> language.text("请求过于频繁，请稍后重新获取验证码。", "Please wait before requesting another code.")
        failure.serviceCode == 50321 -> language.text("邮箱验证暂时不可用，请稍后重试。", "Email verification is temporarily unavailable. Try again later.")
        failure.status == 401 -> language.text("账号或密码不正确，请重试。", "The account or password is incorrect. Please try again.")
        failure.status == 409 -> language.text("资料已经更新，请检查最新内容后再保存。", "Your profile has changed. Review the latest details and save again.")
        failure.status == 429 -> language.text("操作过于频繁，请稍后再试。", "Too many attempts. Please try again later.")
        failure.serviceCode == 4002 -> language.text("账号或邮箱已被使用，请尝试登录。", "This account or email is already in use. Try signing in.")
        failure.status == 413 -> language.text("图片过大，请选择另一张图片。", "The image is too large. Choose another image.")
        failure.status == 400 -> language.text("请检查填写的资料后重试。", "Check the entered details and try again.")
        page == AccountPage.REGISTER && failure.status >= 500 -> language.text(
            "暂时无法确认注册结果。账号可能已创建，请稍后尝试登录。",
            "Registration could not be confirmed. Your account may have been created; try signing in later.",
        )
        else -> language.text("服务暂时不可用，请稍后重试。", "The service is unavailable. Please try again later.")
    }
    is ApiFailure.Network -> if (page == AccountPage.REGISTER) language.text(
        "暂时无法确认注册结果。请检查网络，并尝试登录后再重新注册。",
        "Registration could not be confirmed. Check your connection and try signing in before registering again.",
    ) else if (failure.timedOut) language.text("请求超时，请重试。", "The request timed out. Please try again.")
    else language.text("无法连接，请检查网络后重试。", "Could not connect. Check your connection and try again.")
    is ApiFailure.InvalidInput -> when (failure.field) {
        "usernameAt" -> language.usernameRule()
        "verificationCode" -> language.text("请输入 6 位数字验证码。", "Enter the six-digit verification code.")
        "password" -> language.text("密码至少需要 4 个字符，且不能超过 72 个 UTF-8 字节。", "Use at least 4 characters and no more than 72 UTF-8 bytes for the password.")
        "nickname" -> language.text("昵称不能超过 255 个字符。", "The display name must be at most 255 characters.")
        "pronouns" -> language.text("代词不能超过 64 个字符。", "Pronouns must be at most 64 characters.")
        "signature" -> language.text("个人简介不能超过 200 个字符。", "The bio must be at most 200 characters.")
        "avatar" -> language.text("请重新选择一张有效图片。", "Please choose a valid image again.")
        else -> language.text("请检查填写的资料。", "Please check the entered details.")
    }
    is ApiFailure.SessionRequired -> language.text("登录已过期，请重新登录。", "Your session has expired. Please sign in again.")
    is ApiFailure.SessionChanged -> language.text("账号已改变，请重新操作。", "The account has changed. Please try again.")
    is ApiFailure.SecureStorage -> language.text("无法安全保存登录状态，请重试。", "The session could not be saved securely. Please try again.")
    is ApiFailure.InvalidResponse -> language.text("服务返回了无效响应，请重试。", "The service returned an invalid response. Please try again.")
}
