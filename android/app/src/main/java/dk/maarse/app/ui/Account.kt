package dk.maarse.app.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.ui.Alignment
import androidx.compose.ui.text.style.TextOverflow
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
import dk.maarse.app.hex

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
 * The page title already says "Account and devices", so there is no title here.
 */
@Composable
fun AccountSection() {
    val ctx = LocalContext.current
    val p by Node.person.collectAsState()
    val me by Node.me.collectAsState()
    val err by Node.lastError.collectAsState()
    val state = p?.state
    when (state) {
        null -> Hint("Fetching…")
        PersonState.NONE -> NoAccount()
        PersonState.AWAITING -> {
            GroupTitle("Waiting to be linked")
            Card2 {
                Text("This device is waiting to be linked (10 minutes). On your master device, open Settings → Account and devices → Link a device, and scan this code or enter this address:",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                SelectionContainer { Text(me?.addr ?: "", fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
                me?.addr?.let { a ->
                    val bmp = remember(a) { qrBitmap("maarse:link?addr=$a") }
                    androidx.compose.foundation.layout.Box(Modifier.fillMaxWidth().padding(top = 4.dp), contentAlignment = Alignment.Center) { androidx.compose.foundation.Image(bmp.asImageBitmap(), "QR code to scan on the master device", Modifier.size(220.dp)) }
                }
                Text("Check words: ${p?.words}", fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 4.dp))
                Text("The master device shows the same words when it has linked this device.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { copy(ctx, "Device address", me?.addr ?: "") }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Copy address") }
                TextButton(onClick = { Node.personSetup(PersonOp.CANCEL) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Cancel") }
            }
        }
        PersonState.MEMBER -> Member()
    }
    err?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) }
    Dialogs()
}

@Composable
private fun NoAccount() {
    var restoring by remember { mutableStateOf(false) }
    var words by remember { mutableStateOf("") }
    GroupTitle("Your account")
    Card2 {
        Text("With an account, friends write to one address and the message reaches all your devices (phone, PC …). Your sent messages show on all of them too. MeshCore needs an account as well. The device you create it on becomes the master.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Button(onClick = { Node.personSetup(PersonOp.CREATE) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Create my account") }
        OutlinedButton(onClick = { Node.personSetup(PersonOp.AWAIT_LINK) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Link this device to my account") }
        Text("Already have an account on your computer or another phone? Choose \"Link this device to my account\": two accounts would mean two addresses.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    Spacer(Modifier.height(8.dp))
    Group {
        GroupRow("Restore an account", "From a backup file or 24 words", last = true) { restoring = !restoring }
    }
    if (restoring) Card2(Modifier.padding(top = 8.dp)) {
        val ctx = LocalContext.current
        var file by remember { mutableStateOf<ByteArray?>(null) }
        var code by remember { mutableStateOf("") }
        val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            if (uri != null) file = runCatching { ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
        }
        Text("From a backup file and its 6-word code", style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
        OutlinedButton(onClick = { pick.launch(arrayOf("*/*")) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(if (file == null) "Choose the file" else "File chosen (${file?.size} bytes)") }
        OutlinedTextField(code, { code = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("The 6-word code") })
        Button(onClick = { file?.let { Node.restoreBackup(it, code) } }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), enabled = file != null && code.trim().split(Regex("\\s+")).size == 6) {
            Text("Restore from the file")
        }
        Text("Or from the 24 words", style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
        OutlinedTextField(words, { words = it }, Modifier.fillMaxWidth(), label = { Text("The 24 words, separated by spaces") })
        Button(onClick = { Node.personSetup(PersonOp.RESTORE, words.trim().lowercase()) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), enabled = words.trim().split(Regex("\\s+")).size == 24) {
            Text("Restore from the words")
        }
        Text("The device you restore on becomes the master device.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
    var removing by remember { mutableStateOf<String?>(null) }
    var copied by remember { mutableStateOf(false) }

    // The address friends write to: short here, the whole one is copied.
    GroupTitle("Your account address")
    Card2 {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(Modifier.weight(1f)) {
                Text(shortAddr(pp.person) + "…", fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                Text("Give friends the whole address with Copy.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            OutlinedButton(onClick = { copy(ctx, "Account address", pp.person); copied = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text(if (copied) "Copied" else "Copy") }
        }
        Text(if (pp.master) "This is the master device (device ${pp.index})." else "This is device ${pp.index}.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }

    GroupTitle("Devices")
    Group {
        pp.devices.forEachIndexed { i, (d, revoked) ->
            val name = Node.nodes.value[d]?.name
            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = 14.dp, end = 6.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Column(Modifier.weight(1f)) {
                    Text(name?.let { "~$it" } ?: shortAddr(d), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        buildList { add(shortAddr(d)); if (d == me?.addr) add("this device"); if (revoked) add("removed") }.joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (pp.master && !revoked && d != me?.addr) TextButton(onClick = { removing = d }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Remove", color = MaterialTheme.colorScheme.error) }
            }
            if (i < pp.devices.lastIndex) RowDivider()
        }
    }
    removing?.let { d ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove ${shortAddr(d)}?") },
            text = { Text("Messages to the account will no longer reach it, and the account gets a new inbox key it does not know. Use this for a lost or stolen device.") },
            confirmButton = { TextButton(onClick = { Node.personSetup(PersonOp.REMOVE, d); removing = null }) { Text("Remove", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }

    GroupTitle("Actions")
    Group {
        if (pp.master) {
            GroupRow("Link a device", "Scan its QR code or enter its address") { adding = !adding }
            GroupRow("Make a backup", "A file and a 6-word code") { Node.personSetup(PersonOp.BACKUP) }
        }
        GroupRow("Sync now", "Fetch your mail and copies from your other devices (also done at start-up)", last = true) { Node.personSetup(PersonOp.SYNC) }
    }
    if (adding && pp.master) Card2(Modifier.padding(top = 8.dp)) {
        Text("On the new device: Settings → Account and devices → \"Link this device to my account\". Scan the code it shows, or enter its address.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        val scan = rememberLauncherForActivityResult(com.journeyapps.barcodescanner.ScanContract()) { r ->
            // A scanned code from the new device is linked at once; the check words then show on both devices.
            r.contents?.let {
                val scanned = linkAddr(it)
                if (scanned.length == 32 && scanned.all { c -> c in "0123456789abcdef" }) { Node.linkDevice(scanned); adding = false; addr = "" } else addr = scanned
            }
        }
        Button(onClick = {
            scan.launch(com.journeyapps.barcodescanner.ScanOptions().setPrompt("Scan the new device's QR code").setBeepEnabled(false).setOrientationLocked(false))
        }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Scan QR code") }
        OutlinedTextField(addr, { addr = linkAddr(it) }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("The new device's address (32 characters)") })
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = { Node.linkDevice(addr); adding = false; addr = "" }, modifier = Modifier.heightIn(min = 48.dp), enabled = addr.length == 32 && addr.all { it in "0123456789abcdef" }) { Text("Link") }
            TextButton(onClick = { adding = false }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Cancel") }
        }
    }
    if (!pp.master) Hint("Devices are added on the master device.")
    Spacer(Modifier.height(12.dp))
    Group {
        GroupRow("Remove this device from the account", last = true) { leaving = true }
    }
    if (leaving) AlertDialog(
        onDismissRequest = { leaving = false },
        title = { Text("Remove this device?") },
        text = {
            Text(if (pp.master) "This is the master device. Without it you can only add devices again with the 24 words." else "Messages to the account will no longer reach this device. The device keeps its own address.")
        },
        confirmButton = { TextButton(onClick = { Node.personSetup(PersonOp.LEAVE); leaving = false }) { Text("Remove", color = MaterialTheme.colorScheme.error) } },
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
                Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
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


@Composable
fun MeshCoreAccessSection(setup: Boolean = false) {
    val access by Node.meshcoreAccess.collectAsState()
    var address by remember { mutableStateOf(Node.prefs.meshcoreBackbone) }
    var message by remember { mutableStateOf("") }
    var contact by remember { mutableStateOf("") }
    var directText by remember { mutableStateOf("") }
    var channelName by remember { mutableStateOf("") }
    var channelKey by remember { mutableStateOf("") }
    var channelText by remember { mutableStateOf("") }
    var section by remember { mutableStateOf("Public") }
    var repeaterKey by remember { mutableStateOf("") }
    var repeaterPassword by remember { mutableStateOf("") }
    var repeaterCommand by remember { mutableStateOf("") }
    if (setup) { McSetup(); return }
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (setup || access.key == null) {
            OutlinedTextField(address, { address = it }, label = { Text("Backbone HTTPS address") }, singleLine = true)
            OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 0) }, enabled = !access.busy) { Text("Check access") }
        }
        if (access.text.isNotEmpty()) Text(access.text)
        val key = access.key
        if (key != null) {
            SelectionContainer { Text("MeshCore public key: $key", fontFamily = FontFamily.Monospace) }
            if (setup) {
                Hint("Open MeshCore from Chats to use your channels and radio.")
                return@Column
            }
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                for (item in listOf("Public", "Channels", "Direct")) {
                    TextButton(onClick = { section = item }, enabled = section != item) { if (item == "Public") androidx.compose.material3.Text(item) else Text(item) }   // "Public" is a channel name: never translated
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                for (item in listOf("Nodes", "Repeater")) {
                    TextButton(onClick = { section = item }, enabled = section != item) { Text(item) }
                }
            }
            when (section) {
                "Public" -> {
                    androidx.compose.material3.Text(if (access.unread > 0) "Public (${access.unread} unread)" else "Public", style = MaterialTheme.typography.titleMedium)
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 5) }, enabled = !access.busy) { Text("Refresh messages") }
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 7, "public-read:${access.lastId}") },
                        enabled = access.unread > 0 && !access.busy) { Text("Mark read") }
                    for (entry in access.messages) Text(entry)
                    OutlinedTextField(message, { message = it }, label = { Text("Message to MeshCore Public") }, singleLine = true)
                    val bytes = message.trim().encodeToByteArray()
                    Button(onClick = { Node.checkMeshcoreAccess(address.trim(), 6, "public-send:${bytes.hex()}"); message = "" },
                        enabled = bytes.size in 1..90 && !access.busy) { Text("Send") }
                }
                "Channels" -> {
                    Text("Channels", style = MaterialTheme.typography.titleMedium)
                    OutlinedTextField(channelName, { channelName = it }, label = { Text("Channel name") }, singleLine = true)
                    OutlinedTextField(channelKey, { channelKey = it }, label = { Text("Optional 32-digit hex key") }, singleLine = true,
                        visualTransformation = androidx.compose.ui.text.input.PasswordVisualTransformation())
                    val joinName = channelName.trim().lowercase().removePrefix("#")
                    val joinKey = channelKey.trim()
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 13, "channel-join:$joinName:$joinKey"); channelKey = "" },
                        enabled = !access.busy && joinName.matches(Regex("[a-z0-9-]{1,24}")) && (joinKey.isEmpty() || joinKey.matches(Regex("[a-fA-F0-9]{32}")))) { Text("Join channel") }
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 12) }, enabled = !access.busy) { Text("Refresh channels") }
                    for (channel in access.channels) OutlinedButton(onClick = {
                        Node.checkMeshcoreAccess(address.trim(), 15, "channel-history:$channel")
                    }, enabled = !access.busy) { Text("#$channel") }
                    access.channel?.let { channel ->
                        Text("#${channel.contact}")
                        OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 14, "channel-leave:${channel.contact}") },
                            enabled = !access.busy) { Text("Leave channel") }
                        if (channel.unread > 0) OutlinedButton(onClick = {
                            Node.checkMeshcoreAccess(address.trim(), 17, "channel-read:${channel.contact}:${channel.lastId}")
                        }, enabled = !access.busy) { Text("Mark ${channel.unread} read") }
                        for (entry in channel.messages) Text(entry)
                        OutlinedTextField(channelText, { channelText = it }, label = { Text("Channel message") }, singleLine = true)
                        val channelBytes = channelText.trim().encodeToByteArray()
                        Button(onClick = {
                            Node.checkMeshcoreAccess(address.trim(), 16, "channel-send:${channel.contact}:${channelBytes.hex()}")
                            channelText = ""
                        }, enabled = channelBytes.size in 1..90 && !access.busy) { Text("Send channel message") }
                    }
                }
                "Direct" -> {
                    Text("Direct messages", style = MaterialTheme.typography.titleMedium)
                    OutlinedTextField(contact, { contact = it }, label = { Text("Contact public key") }, singleLine = true)
                    val keyText = contact.trim().lowercase()
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 9, "direct-history:$keyText") },
                        enabled = keyText.matches(Regex("[0-9a-f]{64}")) && !access.busy) { Text("Open conversation") }
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 8) }, enabled = !access.busy) { Text("Find conversations") }
                    for (item in access.contacts) OutlinedButton(onClick = {
                        contact = item
                        Node.checkMeshcoreAccess(address.trim(), 9, "direct-history:$item")
                    }, enabled = !access.busy) { Text(item.take(12)) }
                    access.direct?.let { direct ->
                        Text(direct.contact.take(12))
                        OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 9, "direct-history:${direct.contact}") },
                            enabled = !access.busy) { Text("Refresh conversation") }
                        if (direct.unread > 0) OutlinedButton(onClick = {
                            Node.checkMeshcoreAccess(address.trim(), 11, "direct-read:${direct.contact}:${direct.lastId}")
                        }, enabled = !access.busy) { Text("Mark ${direct.unread} read") }
                        for (entry in direct.messages) Text(entry)
                        OutlinedTextField(directText, { directText = it }, label = { Text("Direct message") }, singleLine = true)
                        val directBytes = directText.trim().encodeToByteArray()
                        Button(onClick = {
                            Node.checkMeshcoreAccess(address.trim(), 10, "direct-send:${direct.contact}:${directBytes.hex()}")
                            directText = ""
                        }, enabled = directBytes.size in 1..60 && !access.busy) { Text("Send direct message") }
                    }
                }
                "Nodes" -> {
                    for ((operation, label) in listOf(2 to "Announce", 3 to "Scan nearby nodes", 4 to "Fetch scan results")) {
                        OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), operation) }, enabled = !access.busy) { Text(label) }
                    }
                    for (result in access.results) Text(result)
                }
                "Repeater" -> {
                    val target = repeaterKey.ifEmpty { access.repeaterKey }.trim().lowercase()
                    val validTarget = target.matches(Regex("[0-9a-f]{64}")) && !access.busy
                    OutlinedTextField(repeaterKey.ifEmpty { access.repeaterKey }, { repeaterKey = it }, label = { Text("Repeater public key") }, singleLine = true)
                    OutlinedTextField(repeaterPassword, { repeaterPassword = it }, label = { Text("Repeater password") }, singleLine = true,
                        visualTransformation = androidx.compose.ui.text.input.PasswordVisualTransformation())
                    val passwordBytes = repeaterPassword.encodeToByteArray()
                    OutlinedButton(onClick = {
                        Node.checkMeshcoreAccess(address.trim(), 18, "repeater-login:$target:${passwordBytes.hex()}")
                        repeaterPassword = ""
                    }, enabled = validTarget && passwordBytes.size in 1..60) { Text("Log in to repeater") }
                    OutlinedTextField(repeaterCommand, { repeaterCommand = it }, label = { Text("Repeater command, e.g. get name") }, singleLine = true)
                    val commandBytes = repeaterCommand.trim().encodeToByteArray()
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 19, "repeater-command:$target:${commandBytes.hex()}") },
                        enabled = validTarget && commandBytes.size in 1..60) { Text("Run command") }
                    OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 20, "repeater-status:$target") }, enabled = validTarget) { Text("Fetch response") }
                    if (access.repeaterStatus.isNotEmpty()) SelectionContainer { Text(access.repeaterStatus) }
                }
            }
        } else if (access.allowed) Button(onClick = { Node.checkMeshcoreAccess(address.trim(), 1) }, enabled = !access.busy) {
            Text("Create MeshCore identity")
        }
    }
}


/** MeshCore setup: where the backbone is, whether this account may use it, and the MeshCore identity. */
@Composable
private fun McSetup() {
    val ctx = LocalContext.current
    val access by Node.meshcoreAccess.collectAsState()
    var address by remember { mutableStateOf(Node.prefs.meshcoreBackbone) }
    var copied by remember { mutableStateOf(false) }
    McGroupTitle("Backbone", "where MeshCore is reached")
    Card2 {
        Text("MeshCore is a separate radio network. Må:rse reaches it through a backbone server.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedTextField(address, { address = it }, Modifier.fillMaxWidth(), label = { Text("Backbone HTTPS address") }, singleLine = true)
        OutlinedButton(onClick = { Node.checkMeshcoreAccess(address.trim(), 0) }, enabled = !access.busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Check access") }
        if (access.text.isNotEmpty()) Text(access.text, style = MaterialTheme.typography.bodySmall)
    }
    val key = access.key
    if (key != null) {
        McGroupTitle("Identity")
        Card2 {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Column(Modifier.weight(1f)) {
                    Text("MeshCore public key", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(key.take(8) + "…" + key.takeLast(4), fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold)
                }
                OutlinedButton(onClick = { copy(ctx, "MeshCore public key", key); copied = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text(if (copied) "Copied" else "Copy") }
            }
        }
        Hint("Your MeshCore chats and channels are in Chats.")
    } else if (access.allowed) {
        McGroupTitle("Identity")
        Card2 {
            Text("This account has no MeshCore identity yet.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Button(onClick = { Node.checkMeshcoreAccess(address.trim(), 1) }, enabled = !access.busy, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Create MeshCore identity") }
        }
    }
}
