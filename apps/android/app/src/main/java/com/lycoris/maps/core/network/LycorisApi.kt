package com.lycoris.maps.core.network

import com.lycoris.maps.core.model.Marker
import com.lycoris.maps.core.model.User
import kotlinx.serialization.Serializable
import okhttp3.MultipartBody
import okhttp3.RequestBody
import retrofit2.Response
import retrofit2.http.Body
import retrofit2.http.DELETE
import retrofit2.http.Header
import retrofit2.http.GET
import retrofit2.http.Multipart
import retrofit2.http.PATCH
import retrofit2.http.POST
import retrofit2.http.Part
import retrofit2.http.Path
import retrofit2.http.Query

@Serializable
data class AuthEnvelope<T>(val code: Int, val message: String = "", val data: T? = null)

// Deliberately not data classes: generated toString must never expose credentials.
@Serializable
class LoginRequest(val username: String, val password: String)

@Serializable
class RegisterRequest(
    val username: String,
    val nickname: String,
    val email: String,
    val password: String,
    val verificationCode: String,
    val website: String = "",
)

@Serializable
class EmailCodeRequest(val email: String, val purpose: String)
@Serializable
class ResetPasswordRequest(val email: String, val verificationCode: String, val newPassword: String)
@Serializable
data class EmailCodeReceipt(val retryAfterSeconds: Int, val expiresInSeconds: Int)

@Serializable
class ChangePasswordRequest(val oldPassword: String, val newPassword: String)

@Serializable
data class UpdateProfileRequest(val nickname: String? = null, val pronouns: String? = null, val signature: String? = null)

@Serializable
data class CreateMarkerRequest(
    val lat: Double,
    val lng: Double,
    val category: String,
    val title: String,
    val description: String? = null,
    val language: String,
    val openTimeStart: String? = null,
    val openTimeEnd: String? = null,
    val clientRequestId: String,
    val venueType: String? = null,
    val categories: List<String>? = null,
    val openingHoursNote: String? = null,
)

@Serializable
data class EditMarkerRequest(
    val category: String,
    val title: String,
    val description: String? = null,
    val language: String,
    val openTimeStart: String? = null,
    val openTimeEnd: String? = null,
    val venueType: String? = null,
    val categories: List<String>? = null,
    val openingHoursNote: String? = null,
)

@Serializable
data class CreateUploadRequest(val clientRequestId: String, val totalBytes: Int, val sha256: String)

@Serializable
data class UploadReceipt(
    val uploadId: String,
    val markerId: Long,
    val totalBytes: Int,
    val receivedBytes: Int,
    val chunkSize: Int,
    val status: String,
)

/** All successful marker responses are raw DTOs; auth alone uses an envelope. */
interface LycorisApi {
    @GET("api/markers/public")
    suspend fun publicMarkers(@Query("lang") language: String): Response<List<Marker>>

    @GET("api/markers/viewport")
    suspend fun viewport(
        @Query("minLat") south: Double,
        @Query("maxLat") north: Double,
        @Query("minLng") west: Double,
        @Query("maxLng") east: Double,
        @Query("categories") categories: String? = null,
        @Query("lang") language: String,
    ): Response<List<Marker>>

    @GET("api/markers/search")
    suspend fun search(@Query("q") query: String, @Query("lang") language: String): Response<List<Marker>>

    @GET("api/markers/nearby")
    suspend fun nearby(
        @Query("lat") lat: Double,
        @Query("lng") lng: Double,
        @Query("radius") radiusMeters: Int,
        @Query("category") category: String,
        @Query("lang") language: String,
    ): Response<List<Marker>>

    @GET("api/markers/{id}")
    suspend fun marker(@Path("id") id: Long, @Query("lang") language: String): Response<Marker>

    @POST("api/login")
    suspend fun login(@Body request: LoginRequest): Response<AuthEnvelope<User>>

    @POST("api/register")
    suspend fun register(@Body request: RegisterRequest): Response<AuthEnvelope<User>>

    @POST("api/auth/email-code")
    suspend fun sendEmailCode(@Body request: EmailCodeRequest, @Header("X-App-Language") language: String): Response<AuthEnvelope<EmailCodeReceipt>>

    @POST("api/auth/reset-password")
    suspend fun resetPassword(@Body request: ResetPasswordRequest): Response<AuthEnvelope<Unit>>

    @GET("api/me")
    suspend fun me(): Response<AuthEnvelope<User>>

    @PATCH("api/me")
    suspend fun updateProfile(@Body request: UpdateProfileRequest): Response<AuthEnvelope<User>>

    @POST("api/me/password")
    suspend fun changePassword(@Body request: ChangePasswordRequest): Response<AuthEnvelope<Unit>>

    @POST("api/logout")
    suspend fun logout(): Response<AuthEnvelope<Unit>>

    @Multipart
    @POST("api/me/avatar")
    suspend fun uploadAvatar(@Part file: MultipartBody.Part): Response<AuthEnvelope<User>>

    @GET("api/markers/me/favorites")
    suspend fun favoriteIds(): Response<List<Long>>

    @GET("api/markers/me/favorites/details")
    suspend fun favoritePlaces(@Query("lang") language: String): Response<List<Marker>>

    @GET("api/markers/me/created")
    suspend fun createdPlaces(@Query("lang") language: String): Response<List<Marker>>

    @POST("api/markers/{id}/favorite")
    suspend fun addFavorite(@Path("id") id: Long): Response<Unit>

    @DELETE("api/markers/{id}/favorite")
    suspend fun removeFavorite(@Path("id") id: Long): Response<Unit>

    @POST("api/markers")
    suspend fun createMarker(@Body request: CreateMarkerRequest, @Query("lang") language: String): Response<Marker>

    @POST("api/markers")
    suspend fun createMarkerBytes(@Body frozenJson: RequestBody, @Query("lang") language: String): Response<Marker>

    @PATCH("api/markers/{id}")
    suspend fun editMarker(@Path("id") id: Long, @Body request: EditMarkerRequest, @Query("lang") language: String): Response<Marker>

    @PATCH("api/markers/{id}")
    suspend fun editMarkerBytes(@Path("id") id: Long, @Body frozenJson: RequestBody, @Query("lang") language: String): Response<Marker>

    @POST("api/markers/{id}/image-uploads")
    suspend fun createUpload(@Path("id") markerId: Long, @Body request: CreateUploadRequest): Response<UploadReceipt>

    @GET("api/markers/{id}/image-uploads/{uploadId}")
    suspend fun uploadStatus(@Path("id") markerId: Long, @Path("uploadId") uploadId: String): Response<UploadReceipt>

    @POST("api/markers/{id}/image-uploads/{uploadId}/chunks/{offset}")
    suspend fun uploadChunk(
        @Path("id") markerId: Long,
        @Path("uploadId") uploadId: String,
        @Path("offset") offset: Int,
        @Body bytes: RequestBody,
    ): Response<UploadReceipt>

    @POST("api/markers/{id}/image-uploads/{uploadId}/complete")
    suspend fun completeUpload(@Path("id") markerId: Long, @Path("uploadId") uploadId: String): Response<UploadReceipt>
}
