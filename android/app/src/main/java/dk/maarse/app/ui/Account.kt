package dk.maarse.app.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.foundation.layout.size
import dk.maarse.app.Node
import dk.maarse.app.PersonOp
import dk.maarse.app.PersonState
import dk.maarse.app.shortAddr

/** A QR code as a bitmap (zxing), black on white so any camera reads it. */
private fun qrBitmap(text: String, size: Int = 512): android.graphics.Bitmap {
    val m = com.google.zxing.qrcode.QRCodeWriter().encode(text, com.google.zxing.BarcodeFormat.QR_CODE, size, size)
    val bmp = android.graphics.Bitmap.createBitmap(size, size, android.graphics.Bitmap.Config.RGB_565)
    for (x in 0 until size) for (y in 0 until size) bmp.setPixel(x, y, if (m[x, y]) android.graphics.Color.BLACK else android.graphics.Color.WHITE)
    return bmp
}

/** The address in a scanned "maarse:link?addr=…", or a typed one. */
private fun linkAddr(s: String): String =
    Regex("[0-9a-f]{32}").find(s.lowercase())?.value ?: s.trim().lowercase()

private fun copy(ctx: Context, label: String, text: String) {
    ctx.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(label, text))
}

/**
 * Linked devices: one account (person) for the phone, the PC and more. The
 * account's address is what friends write to; every device gets the messages.
 */
@Composable
fun AccountSection() {
    val ctx = LocalContext.current
    val p by Node.person.collectAsState()
    val me by Node.me.collectAsState()
    val err by Node.lastError.collectAsState()
    SectionTitle("Account (linked devices)")
    val state = p?.state
    when (state) {
        null -> Hint("Fetching…")
        PersonState.NONE -> NoAccount()
        PersonState.AWAITING -> {
            Hint("This device is waiting to be linked (10 minutes). On your master device, open Account → Add device and enter this address:")
            Card(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)) {
                Column(Modifier.padding(12.dp)) {
                    SelectionContainer { Text(me?.addr ?: "", fontFamily = FontFamily.Monospace) }
                    me?.addr?.let { a ->
                        val bmp = remember(a) { qrBitmap("maarse:link?addr=$a") }
                        androidx.compose.foundation.Image(bmp.asImageBitmap(), "QR code to scan on the master device", Modifier.padding(top = 8.dp).size(220.dp))
                    }
                    Text("Check words: ${p?.words}", fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 6.dp))
                    Text("The master device shows the same words when it has linked this device.", style = MaterialTheme.typography.bodySmall)
                }
            }
            Row(Modifier.padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { copy(ctx, "Device address", me?.addr ?: "") }) { Text("Copy address") }
                TextButton(onClick = { Node.personSetup(PersonOp.CANCEL) }) { Text("Cancel") }
            }
        }
        PersonState.MEMBER -> Member()
    }
    err?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
    Dialogs()
}

@Composable
private fun NoAccount() {
    var restoring by remember { mutableStateOf(false) }
    var words by remember { mutableStateOf("") }
    Hint("With an account, friends write to one address and the message reaches all your devices (phone, PC …). Your sent messages show on all of them too.")
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Button(onClick = { Node.personSetup(PersonOp.CREATE) }) { Text("Create an account (this is the master device)") }
        OutlinedButton(onClick = { Node.personSetup(PersonOp.AWAIT_LINK) }) { Text("Link this device to my account") }
        TextButton(onClick = { restoring = !restoring }) { Text("Restore an account (backup file or 24 words)") }
        if (restoring) {
            val ctx = LocalContext.current
            var file by remember { mutableStateOf<ByteArray?>(null) }
            var code by remember { mutableStateOf("") }
            val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
                if (uri != null) file = runCatching { ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            }
            Text("From a backup file and its 6-word code:", style = MaterialTheme.typography.labelLarge)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { pick.launch(arrayOf("*/*")) }) { Text(if (file == null) "Choose the file" else "File chosen (${file?.size} bytes)") }
            }
            OutlinedTextField(code, { code = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("The 6-word code") })
            Button(onClick = { file?.let { Node.restoreBackup(it, code) } }, enabled = file != null && code.trim().split(Regex("\\s+")).size == 6) {
                Text("Restore from the file (this becomes the master device)")
            }
            Text("Or from the 24 words:", style = MaterialTheme.typography.labelLarge)
            OutlinedTextField(words, { words = it }, Modifier.fillMaxWidth(), label = { Text("The 24 words, separated by spaces") })
            Button(onClick = { Node.personSetup(PersonOp.RESTORE, words.trim().lowercase()) }, enabled = words.trim().split(Regex("\\s+")).size == 24) {
                Text("Restore (this becomes the master device)")
            }
        }
    }
}

@Composable
private fun Member() {
    val ctx = LocalContext.current
    val p by Node.person.collectAsState()
    val me by Node.me.collectAsState()
    val pp = p ?: return
    var adding by remember { mutableStateOf(false) }
    var addr by remember { mutableStateOf("") }
    var leaving by remember { mutableStateOf(false) }
    Hint("Your account address — give this to friends:")
    Card(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)) {
        Column(Modifier.padding(12.dp)) {
            SelectionContainer { Text(pp.person, fontFamily = FontFamily.Monospace) }
            Text(if (pp.master) "This is the master device (device ${pp.index})." else "This is device ${pp.index}.", style = MaterialTheme.typography.bodySmall)
        }
    }
    OutlinedButton(onClick = { copy(ctx, "Account address", pp.person) }, modifier = Modifier.padding(horizontal = 16.dp)) { Text("Copy account address") }
    Text("Devices", Modifier.padding(start = 16.dp, top = 8.dp), style = MaterialTheme.typography.labelLarge)
    var removing by remember { mutableStateOf<String?>(null) }
    pp.devices.forEach { (d, revoked) ->
        val name = Node.nodes.value[d]?.name
        Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 16.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
            Text(
                buildString {
                    append(shortAddr(d))
                    name?.let { append(" · ~$it") }
                    if (d == me?.addr) append(" · this device")
                    if (revoked) append(" · removed")
                },
                Modifier.weight(1f),
            )
            if (pp.master && !revoked && d != me?.addr) TextButton(onClick = { removing = d }) { Text("Remove") }
        }
    }
    removing?.let { d ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove ${shortAddr(d)}?") },
            text = { Text("Messages to the account will no longer reach it, and the account gets a new inbox key it does not know. Use this for a lost or stolen device.") },
            confirmButton = { TextButton(onClick = { Node.personSetup(PersonOp.REMOVE, d); removing = null }) { Text("Remove") } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
    if (pp.master) {
        if (!adding) OutlinedButton(onClick = { adding = true }, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { Text("Add device") }
        else Column(Modifier.padding(horizontal = 16.dp)) {
            Hint("On the new device: Settings → Account → \"Link this device to my account\". Scan its QR code, or enter the address it shows:")
            val scan = rememberLauncherForActivityResult(com.journeyapps.barcodescanner.ScanContract()) { r ->
                r.contents?.let { addr = linkAddr(it) }
            }
            OutlinedButton(onClick = {
                scan.launch(com.journeyapps.barcodescanner.ScanOptions().setPrompt("Scan the new device's QR code").setBeepEnabled(false).setOrientationLocked(false))
            }) { Text("Scan QR code") }
            OutlinedTextField(addr, { addr = linkAddr(it) }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("The new device's address (32 characters)") })
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { Node.linkDevice(addr); adding = false; addr = "" }, enabled = addr.length == 32 && addr.all { it in "0123456789abcdef" }) { Text("Link") }
                TextButton(onClick = { adding = false }) { Text("Cancel") }
            }
        }
        OutlinedButton(onClick = { Node.personSetup(PersonOp.BACKUP) }, modifier = Modifier.padding(horizontal = 16.dp)) { Text("Make a backup (file + 6-word code)") }
    } else Hint("Devices are added on the master device.")
    TextButton(onClick = { leaving = true }, modifier = Modifier.padding(horizontal = 8.dp)) { Text("Remove this device from the account") }
    if (leaving) AlertDialog(
        onDismissRequest = { leaving = false },
        title = { Text("Remove this device?") },
        text = {
            Text(if (pp.master) "This is the master device. Without it you can only add devices again with the 24 words." else "Messages to the account will no longer reach this device. The device keeps its own address.")
        },
        confirmButton = { TextButton(onClick = { Node.personSetup(PersonOp.LEAVE); leaving = false }) { Text("Remove") } },
        dismissButton = { TextButton(onClick = { leaving = false }) { Text("Cancel") } },
    )
}

@Composable
private fun Dialogs() {
    val ctx = LocalContext.current
    val backup by Node.backupCode.collectAsState()
    var saved by remember { mutableStateOf<String?>(null) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        if (uri != null) Node.readBackup { b ->
            saved = if (b != null && runCatching { ctx.contentResolver.openOutputStream(uri)?.use { it.write(b) } }.isSuccess) "Saved." else "Could not save the file."
        }
    }
    backup?.let { code ->
        AlertDialog(
            onDismissRequest = {},
            title = { Text("Backup made") },
            text = {
                Column {
                    Text("Save the file somewhere safe (another device, a USB stick, cloud storage). It opens only with this code — write it down, it is shown only now:")
                    Text(code, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(vertical = 8.dp))
                    OutlinedButton(onClick = { save.launch("maarse-account.maarse") }) { Text("Save the backup file") }
                    saved?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
                }
            },
            confirmButton = { TextButton(onClick = { Node.backupCode.value = null; saved = null }) { Text("I have saved the file and the code") } },
        )
    }
    val phrase by Node.phrase.collectAsState()
    val linked by Node.linked.collectAsState()
    phrase?.let { words ->
        AlertDialog(
            onDismissRequest = {},
            title = { Text("Write down these 24 words") },
            text = {
                Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                    Text("They are your account. With them you can make a new master device if this one is lost. They are shown only now; anyone who has them owns the account.")
                    words.split(" ").chunked(2).forEachIndexed { row, ws ->
                        Text(ws.mapIndexed { i, w -> "%2d. %-10s".format(row * 2 + i + 1, w) }.joinToString(" "), fontFamily = FontFamily.Monospace, modifier = Modifier.padding(top = 6.dp))
                    }
                }
            },
            confirmButton = { TextButton(onClick = { Node.phrase.value = null }) { Text("I have written them down") } },
        )
    }
    linked?.let { l ->
        AlertDialog(
            onDismissRequest = { Node.linked.value = null },
            title = { Text("Device linked") },
            text = { Text("Device ${l.index} (${shortAddr(l.device)}) belongs to the account now.\n\nCheck words: ${l.words}\n\nThe new device showed the same words while it waited. If they differ, remove the device.") },
            confirmButton = { TextButton(onClick = { Node.linked.value = null }) { Text("OK") } },
        )
    }
}
