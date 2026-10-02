package com.lycoris.maps.feature.contributions

import android.app.TimePickerDialog
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.lycoris.maps.core.model.Language
import com.lycoris.maps.core.model.VenueType
import com.lycoris.maps.core.model.PlaceCategory
import com.lycoris.maps.feature.places.categoryName
import java.util.Locale

/** Native Material form for screens without a complete Android Figma frame. */
@Composable
fun ContributionPanel(
    draft: ContributionDraft?, language: Language,
    command: (suspend () -> Unit) -> Unit,
    coordinator: ContributionCoordinator,
) {
    val zh = language == Language.ZH
    val storageProblem by coordinator.storageProblem.collectAsStateWithLifecycle()
    if (draft == null) {
        Text(if (zh) "草稿不可用，请返回重试。" else "This draft is unavailable. Return and try again.", Modifier.padding(horizontal = 30.dp))
        return
    }
    key(draft.id) {
        var fields by remember { mutableStateOf(draft.fields) }
        var venuesOpen by remember { mutableStateOf(false) }
        var confirmDiscard by remember { mutableStateOf(false) }
        val context = LocalContext.current
        // Coordinator owns the ordered queue so leaving the form does not cancel the final keystroke.
        fun change(next: ContributionFields) { fields = next; coordinator.enqueueFields(draft.id, next) }
        val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia()) { uris ->
            if (uris.isNotEmpty()) command { uris.forEach { coordinator.importPhoto(draft.id, it, append = true) } }
        }
        // Replacing one rejected upload must leave the remaining queue intact.
        val replacementPicker = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
            if (uri != null) command { coordinator.importPhoto(draft.id, uri, append = false) }
        }
        fun time(start: Boolean) {
            val value = if (start) fields.openTimeStart else fields.openTimeEnd
            val parts = value.split(':')
            TimePickerDialog(context, { _, hour, minute ->
                val result = String.format(Locale.ROOT, "%02d:%02d", hour, minute)
                change(if (start) fields.copy(openTimeStart = result) else fields.copy(openTimeEnd = result))
            }, parts.getOrNull(0)?.toIntOrNull() ?: 9, parts.getOrNull(1)?.toIntOrNull() ?: 0, true).show()
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 30.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            storageProblem?.let { Text(draftProblemMessage(it, zh), color = MaterialTheme.colorScheme.error) }
            Text(String.format(Locale.ROOT, "%.6f, %.6f", draft.latitude, draft.longitude), style = MaterialTheme.typography.bodySmall)
            OutlinedTextField(fields.title, { change(fields.copy(title = it)) }, Modifier.fillMaxWidth(), enabled = draft.editable,
                label = { Text(if (zh) "点位名称" else "Place name") }, singleLine = true, keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next))
            PlaceCategory.entries.forEach { category ->
                val selected = category.wireValue in fields.selectedCategories
                Row(Modifier.fillMaxWidth(), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Checkbox(selected, { checked -> change(fields.withCategories(if (checked) fields.selectedCategories + category.wireValue else fields.selectedCategories.filter { it != category.wireValue })) }, enabled = draft.editable)
                    Text(categoryName(category.wireValue, zh), Modifier.weight(1f))
                    if (selected && category.wireValue != fields.selectedCategories.first()) TextButton({ change(fields.withCategories(listOf(category.wireValue) + fields.selectedCategories.filter { it != category.wireValue })) }, enabled = draft.editable) {
                        Text(if (zh) "设为首选" else "Make primary")
                    }
                }
            }
            if (PlaceCategory.ACCESSIBLE_TOILET.wireValue in fields.selectedCategories) {
                Text(if (zh) "场所类型" else "Venue type", style = MaterialTheme.typography.labelLarge)
                Box {
                    OutlinedButton({ venuesOpen = true }, Modifier.fillMaxWidth(), enabled = draft.editable) {
                        Text((VenueType.fromWire(fields.venueType) ?: VenueType.OTHER).label(zh))
                    }
                    DropdownMenu(venuesOpen, { venuesOpen = false }) {
                        VenueType.entries.forEach { venue -> DropdownMenuItem(text = { Text(venue.label(zh)) }, onClick = {
                            change(fields.copy(venueType = venue.wireValue)); venuesOpen = false
                        }) }
                    }
                }
            }
            OutlinedTextField(fields.description, { change(fields.copy(description = it)) }, Modifier.fillMaxWidth(), enabled = draft.editable,
                label = { Text(if (zh) "描述" else "Description") }, minLines = 3, maxLines = 8)
            Text(if (zh) "开放时间（可选）" else "Opening hours (optional)", style = MaterialTheme.typography.labelLarge)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton({ time(true) }, Modifier.weight(1f), enabled = draft.editable) { Text(fields.openTimeStart.ifEmpty { if (zh) "开始" else "From" }) }
                OutlinedButton({ time(false) }, Modifier.weight(1f), enabled = draft.editable) { Text(fields.openTimeEnd.ifEmpty { if (zh) "结束" else "To" }) }
                if (fields.openTimeStart.isNotEmpty() || fields.openTimeEnd.isNotEmpty()) IconButton({ change(fields.copy(openTimeStart = "", openTimeEnd = "")) }, enabled = draft.editable) { Icon(Icons.Rounded.Close, if (zh) "清除时间" else "Clear hours") }
            }
            OutlinedTextField(fields.openingHoursNote.orEmpty(), { change(fields.copy(openingHoursNote = it)) }, Modifier.fillMaxWidth(), enabled = draft.editable,
                label = { Text(if (zh) "营业时间备注" else "Opening hours note") }, minLines = 2, maxLines = 5)
            draft.remainingPhotos.forEachIndexed { index, photo ->
                Row(Modifier.fillMaxWidth(), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Text("${draft.uploadedPhotoCount + index + 1}. ${photo.width} × ${photo.height}", Modifier.weight(1f))
                    if (draft.editable) TextButton({ command { coordinator.removePhoto(draft.id, photo.id) } }) { Text(if (zh) "移除图片" else "Remove photo") }
                }
            }
            if (draft.canReplacePhoto) OutlinedButton({
                val request = PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)
                if (draft.editable) picker.launch(request) else replacementPicker.launch(request)
            }, Modifier.fillMaxWidth()) {
                Text(if (zh) { if (draft.editable) "添加图片" else "更换图片" } else { if (draft.editable) "Add photos" else "Replace photo" })
            }
            when (draft.phase) {
                DraftPhase.DRAFT -> {
                    Text(if (zh) "草稿自动保存。提交后需要审核。" else "Your draft is saved automatically. Submissions are reviewed.", style = MaterialTheme.typography.bodySmall)
                    Button({ command { coordinator.submit(draft.id) } }, Modifier.fillMaxWidth(),
                        enabled = fields.isValid() && (draft.original == null || fields != ContributionFields.fromMarker(draft.original) || draft.photo != null)) { Text(if (zh) "提交" else "Submit") }
                }
                DraftPhase.COMPLETE -> Text(if (zh) "已提交，等待审核。" else "Submitted for review.")
                DraftPhase.UNCERTAIN_EDIT -> Text(if (zh) "提交结果尚不确定，请先查看我的点位确认；为避免重复，不会自动重发。" else "The submission result is uncertain. Check My places before editing again; this request will not be resent automatically.")
                else -> {
                    Text(if (zh) "正在提交，网络恢复后会继续。" else "Submitting. This will continue when the connection returns.")
                    draft.upload?.let { receipt -> LinearProgressIndicator(progress = { receipt.receivedBytes.toFloat() / receipt.totalBytes.coerceAtLeast(1) }, modifier = Modifier.fillMaxWidth()) }
                }
            }
            draft.problem?.let { Text(draftProblemMessage(it, zh), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium) }
            if (draft.safelyResumable && draft.problem != null && draft.problem !in setOf(DraftProblem.INACCESSIBLE, DraftProblem.PHOTO_EXPIRED, DraftProblem.PHOTO_REJECTED, DraftProblem.MISSING_PHOTO)) {
                TextButton({ command { coordinator.retry(draft.id) } }) { Text(if (zh) "重试" else "Retry") }
            }
            if (draft.editable || draft.phase == DraftPhase.COMPLETE) TextButton({ confirmDiscard = true }) { Text(if (zh) "移除本机草稿" else "Remove local draft") }
        }
        if (confirmDiscard) AlertDialog(onDismissRequest = { confirmDiscard = false },
            text = { Text(if (zh) "移除这份本机草稿？已经提交的内容不受影响。" else "Remove this local draft? Submitted content will remain unchanged.") },
            confirmButton = { TextButton({ confirmDiscard = false; command { coordinator.discard(draft.id) } }) { Text(if (zh) "移除" else "Remove") } },
            dismissButton = { TextButton({ confirmDiscard = false }) { Text(if (zh) "取消" else "Cancel") } })
    }
}

internal fun draftProblemMessage(problem: DraftProblem, zh: Boolean): String = when (problem) {
    DraftProblem.SESSION_REQUIRED -> if (zh) "请用创建草稿的账号重新登录。" else "Sign in with the account that created this draft."
    DraftProblem.NETWORK -> if (zh) "网络暂时不可用，已保存进度。" else "Connection unavailable. Progress has been saved."
    DraftProblem.RETRY_LIMIT -> if (zh) "自动重试已暂停，可以稍后手动重试。" else "Automatic retries are paused. You can retry later."
    DraftProblem.PHOTO_EXPIRED, DraftProblem.PHOTO_REJECTED, DraftProblem.MISSING_PHOTO -> if (zh) "需要重新选择图片，文字内容已保留。" else "Select the photo again. Your text has been retained."
    DraftProblem.INVALID_FIELDS -> if (zh) "请检查名称、分类和开放时间。" else "Check the name, category, and opening hours."
    DraftProblem.INACCESSIBLE -> if (zh) "这个点位暂时不可访问。" else "This place is currently inaccessible."
    DraftProblem.UNCERTAIN_EDIT -> if (zh) "需要确认上次编辑结果。" else "The previous edit needs confirmation."
    else -> if (zh) "无法继续提交。草稿已保留，请稍后重试。" else "Could not continue. The draft has been retained; please retry later."
}
