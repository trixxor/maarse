//! Backbone administrator grants, keyed by Må:rse person address.

use std::{collections::BTreeSet, fs, io, path::Path, sync::Mutex};

static GRANTS: Mutex<()> = Mutex::new(());

fn person_address(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn read_grants(dir: &Path) -> io::Result<BTreeSet<String>> {
    match fs::read_to_string(dir.join("meshcore-access.txt")) {
        Ok(text) => text
            .lines()
            .map(|value| {
                person_address(value).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid MeshCore grant")
                })
            })
            .collect(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(err) => Err(err),
    }
}

pub fn set_grant(dir: &Path, person: &str, allowed: bool) -> io::Result<()> {
    let person = person_address(person).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Enter a 32-character hexadecimal person address.",
        )
    })?;
    let _guard = GRANTS.lock().unwrap_or_else(|err| err.into_inner());
    let mut grants = read_grants(dir)?;
    if allowed {
        grants.insert(person);
    } else {
        grants.remove(&person);
    }
    let pending = dir.join("meshcore-access.tmp");
    fs::write(&pending, grants.into_iter().collect::<Vec<_>>().join("\n"))?;
    fs::rename(pending, dir.join("meshcore-access.txt"))
}

// Keep the highest verified list locally so an older, valid signature cannot
// re-enable a revoked device after this backbone has seen the newer list.
fn authorize_device(
    dir: &Path,
    proof: &maarse_crypto::meshcore_access::AuthorizedDevice,
) -> io::Result<()> {
    let _guard = GRANTS.lock().unwrap_or_else(|err| err.into_inner());
    let person = proof.person.to_string();
    if !read_grants(dir)?.contains(&person) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "MeshCore access is not granted",
        ));
    }
    let lists = dir.join("meshcore-device-lists");
    fs::create_dir_all(&lists)?;
    let path = lists.join(&person);
    let current = proof.list.to_bytes();
    match fs::read(&path) {
        Ok(saved) => {
            let version = saved
                .get(..4)
                .and_then(|bytes| bytes.try_into().ok())
                .map(u32::from_le_bytes)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid saved device list")
                })?;
            if proof.list.version < version
                || (proof.list.version == version && saved != current.as_slice())
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "stale or conflicting device list",
                ));
            }
            if proof.list.version == version {
                return Ok(());
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let pending = path.with_extension("tmp");
    fs::write(&pending, current.as_slice())?;
    fs::rename(pending, path)
}

/// Person addresses that may use MeshCore access, sorted.
pub fn grants(dir: &Path) -> io::Result<Vec<String>> {
    let _guard = GRANTS.lock().unwrap_or_else(|err| err.into_inner());
    Ok(read_grants(dir)?.into_iter().collect())
}

#[derive(serde::Deserialize)]
struct DeviceProof {
    device: String,
    certificate: String,
    list: String,
    signature: String,
    operation: Option<String>,
}

fn unhex(value: &str) -> Option<Vec<u8>> {
    if !value.is_ascii() || !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).ok())
        .collect()
}

pub fn session(stream: impl io::Read + io::Write, dir: &Path) {
    use maarse_crypto::{PublicIdentity, meshcore_access::verify_request, person::LIST_MAX_LEN};
    use rand_core::{OsRng, RngCore};
    use tungstenite::{Message, protocol::WebSocketConfig};

    let config = WebSocketConfig {
        max_message_size: Some(2048),
        max_frame_size: Some(2048),
        ..Default::default()
    };
    let Ok(mut socket) = tungstenite::accept_with_config(stream, Some(config)) else {
        return;
    };
    let mut challenge = [0u8; 32];
    OsRng.fill_bytes(&mut challenge);
    let challenge_hex: String = challenge.iter().map(|byte| format!("{byte:02x}")).collect();
    let hello = serde_json::json!({"challenge": challenge_hex, "version": 1});
    if socket.send(Message::Text(hello.to_string())).is_err() {
        return;
    }
    let Ok(Message::Text(response)) = socket.read() else {
        return;
    };
    let proof = (|| {
        let response: DeviceProof = serde_json::from_str(&response).ok()?;
        let public: [u8; maarse_crypto::PUBLIC_IDENTITY_LEN] =
            unhex(&response.device)?.try_into().ok()?;
        let device = PublicIdentity::from_bytes(&public)?;
        let certificate = unhex(&response.certificate)?.try_into().ok()?;
        let signature = unhex(&response.signature)?.try_into().ok()?;
        let list = unhex(&response.list)?;
        if list.len() > LIST_MAX_LEN {
            return None;
        }
        let operation = response.operation.as_deref().unwrap_or("access");
        if !valid_operation(operation) {
            return None;
        }
        let proof = verify_request(
            &device,
            &certificate,
            &list,
            &challenge,
            operation.as_bytes(),
            &signature,
        )
        .ok()?;
        authorize_device(dir, &proof).ok()?;
        Some((proof, operation.to_owned()))
    })();
    let reply = match proof {
        Some((proof, operation)) => {
            let person = proof.person.to_string();
            match radio_operation(dir, &person, &operation) {
                Ok(mut reply) => {
                    // So a client can tell "backbone reachable" from "radio gateway connected".
                    let gateway = crate::meshcore_gateway::status();
                    reply["gateway"] = serde_json::json!({
                        "connected": gateway["gatewayConnected"],
                        "worker": gateway["workerConnected"],
                    });
                    reply
                }
                Err(_) => {
                    serde_json::json!({"allowed": true, "person": person, "error": "Radio worker is unavailable"})
                }
            }
        }
        None => serde_json::json!({"allowed": false}),
    };
    let _ = socket.send(Message::Text(reply.to_string()));
    let _ = socket.close(None);
}

fn valid_operation(operation: &str) -> bool {
    if matches!(
        operation,
        "access"
            | "ensure"
            | "announce"
            | "scan"
            | "scan-results"
            | "public-history"
            | "direct-list"
            | "channel-list"
            | "overview"
            | "nodes"
    ) {
        return true;
    }
    if let Some((verb, rest)) = operation.split_once(':') {
        if verb == "set-name" {
            return valid_message(rest) && unhex(rest).is_some_and(|name| name.len() <= 31);
        }
        if verb == "favorite" {
            let (key, value) = rest.split_once(':').unwrap_or((rest, ""));
            return direct_key(key) && matches!(value, "0" | "1");
        }
        if matches!(
            verb,
            "repeater-login" | "repeater-command" | "repeater-status"
        ) {
            let (key, value) = rest.split_once(':').unwrap_or((rest, ""));
            return direct_key(key)
                && if verb == "repeater-status" {
                    value.is_empty()
                } else {
                    value.len() <= 120 && valid_message(value)
                };
        }
        if verb.starts_with("channel-") {
            let (name, argument) = rest.split_once(':').unwrap_or((rest, ""));
            return channel_name(name)
                && match verb {
                    "channel-join" => {
                        argument.is_empty() || (argument.len() == 32 && unhex(argument).is_some())
                    }
                    "channel-leave" | "channel-history" => argument.is_empty(),
                    "channel-read" => argument.parse::<u64>().is_ok(),
                    "channel-send" => argument.len() <= 180 && valid_message(argument),
                    _ => false,
                };
        }
        if matches!(verb, "direct-history" | "direct-send" | "direct-read") {
            let (key, argument) = rest.split_once(':').unwrap_or((rest, ""));
            return direct_key(key)
                && match verb {
                    "direct-history" => argument.is_empty(),
                    "direct-read" => argument.parse::<u64>().is_ok(),
                    _ => argument.len() <= 120 && valid_message(argument),
                };
        }
    }
    if let Some(body) = operation.strip_prefix("public-send:") {
        return body.len() <= 180 && valid_message(body);
    }
    operation
        .strip_prefix("public-read:")
        .is_some_and(|id| id.parse::<u64>().is_ok())
}

fn channel_name(name: &str) -> bool {
    (1..=24).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn channels(node: &Path) -> io::Result<std::collections::BTreeMap<String, String>> {
    match fs::read_to_string(node.join("channels.txt")) {
        Ok(data) => Ok(data
            .lines()
            .filter_map(|line| line.split_once(' '))
            .filter(|(name, key)| channel_name(name) && key.len() == 32 && unhex(key).is_some())
            .map(|(name, key)| (name.into(), key.into()))
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(error),
    }
}

fn direct_key(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_message(value: &str) -> bool {
    unhex(value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|message| !message.trim().is_empty() && !message.chars().any(char::is_control))
}

fn conversation_history(
    node: &Path,
    name: &str,
) -> io::Result<(std::collections::VecDeque<serde_json::Value>, u64, u64)> {
    use std::io::BufRead;
    let last_read = match fs::read_to_string(node.join(format!("{name}-read.txt"))) {
        Ok(value) => value.parse::<u64>().unwrap_or(0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let mut recent = std::collections::VecDeque::with_capacity(100);
    let mut unread = 0;
    let mut last_id = 0;
    let log = match fs::File::open(node.join(format!("{name}-history.txt"))) {
        Ok(log) => log,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((recent, unread, last_id));
        }
        Err(error) => return Err(error),
    };
    for (index, line) in std::io::BufReader::new(log).lines().enumerate() {
        let line = line?;
        let mut fields = line.splitn(3, ' ');
        let (Some(direction), Some(timestamp), Some(body)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some(text) = unhex(body).and_then(|bytes| String::from_utf8(bytes).ok()) else {
            continue;
        };
        let Ok(timestamp) = timestamp.parse::<u32>() else {
            continue;
        };
        if !matches!(direction, "in" | "out") {
            continue;
        }
        let id = index as u64 + 1;
        if direction == "in" && id > last_read {
            unread += 1;
        }
        if recent.len() == 100 {
            recent.pop_front();
        }
        recent.push_back(serde_json::json!({"id": id, "text": text, "timestamp": timestamp, "mine": direction == "out"}));
        last_id = id;
    }
    // What happened to our messages (heard.txt: "<time> <repeaters that passed it on> <best SNR> <acknowledged>").
    if let Ok(heard) = fs::read_to_string(node.join(format!("{name}-heard.txt"))) {
        let heard: std::collections::HashMap<u64, (u64, f64, bool)> = heard
            .lines()
            .filter_map(|line| {
                let mut fields = line.split(' ');
                let time = fields.next()?.parse().ok()?;
                let repeats = fields.next()?.parse().ok()?;
                let snr = fields.next()?.parse().ok()?;
                Some((time, (repeats, snr, fields.next()? == "1")))
            })
            .collect();
        for message in recent.iter_mut().filter(|m| m["mine"] == true) {
            if let Some(&(repeats, snr, delivered)) = message["timestamp"].as_u64().and_then(|time| heard.get(&time)) {
                message["repeats"] = repeats.into();
                message["delivered"] = delivered.into();
                if repeats > 0 {
                    message["snr"] = snr.into();
                }
            }
        }
    }
    Ok((recent, unread, last_id))
}

fn radio_operation(dir: &Path, person: &str, operation: &str) -> io::Result<serde_json::Value> {
    let key = provision(dir, person, operation == "ensure")?;
    let mut reply = serde_json::json!({"allowed": true, "person": person, "publicKey": key});
    if key.is_none() || matches!(operation, "access" | "ensure") {
        return Ok(reply);
    }
    let node = dir.join("meshcore-nodes").join(person);
    match operation {
        "announce" | "scan" => {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(node.join(operation))?;
            reply["queued"] = operation.into();
        }
        "scan-results" => {
            if node.join("scan").exists() {
                reply["scanning"] = true.into();
            } else {
                match fs::read_to_string(node.join("scan-results.txt")) {
                    Ok(lines) => {
                        reply["results"] =
                            serde_json::json!(lines.lines().take(100).collect::<Vec<_>>())
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {
                        reply["results"] = serde_json::json!([])
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        operation if operation.starts_with("repeater-") => {
            let mut fields = operation.split(':');
            let verb = fields.next().unwrap();
            let key = fields.next().unwrap().to_ascii_lowercase();
            reply["repeater"] = key.as_str().into();
            if verb == "repeater-status" {
                let status = node.join(format!("repeater-{key}.txt"));
                match fs::read(status) {
                    Ok(text) => {
                        reply["repeaterStatus"] = String::from_utf8_lossy(&text).into_owned().into()
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        reply["repeaterStatus"] = "Login required.".into()
                    }
                    Err(error) => return Err(error),
                }
            } else {
                let _guard = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
                let job = node.join("repeater-job.txt");
                if job.exists() {
                    reply["error"] = "A repeater request is already queued.".into();
                    return Ok(reply);
                }
                let value = unhex(fields.next().unwrap()).ok_or(io::ErrorKind::InvalidInput)?;
                let mut record = format!("{verb}\n{key}\n").into_bytes();
                record.extend(value);
                let pending = job.with_extension("tmp");
                fs::write(&pending, record)?;
                fs::rename(pending, &job)?;
                if verb == "repeater-login" {
                    for _ in 0..10 {
                        if !job.exists() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    match fs::remove_file(job) {
                        Ok(()) => {
                            reply["error"] =
                                "Radio worker did not accept the login request.".into();
                            return Ok(reply);
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                reply["queued"] = verb.into();
            }
        }
        "overview" => {
            let names = node_names(&node);
            let mut conversations = vec![conversation_summary(&node, "public", "public", "public", "Public")?];
            for channel in channels(&node)?.into_keys() {
                let title = format!("#{channel}");
                conversations.push(conversation_summary(&node, &format!("channel-{channel}"), "channel", &channel, &title)?);
            }
            for key in direct_conversations(&node)? {
                let title = names.get(&key).cloned().unwrap_or_else(|| format!("{}…", &key[..8]));
                conversations.push(conversation_summary(&node, &format!("direct-{key}"), "direct", &key, &title)?);
            }
            // Public stays first; the rest by their latest message.
            conversations[1..].sort_by(|a, b| b["time"].as_u64().cmp(&a["time"].as_u64()));
            let unread: u64 = conversations.iter().filter_map(|c| c["unread"].as_u64()).sum();
            reply["conversations"] = serde_json::json!(conversations);
            reply["totalUnread"] = unread.into();
        }
        "nodes" => {
            let names = node_names(&node);
            let favourites = favorites(&node)?;
            let mut nodes = heard_nodes(&node, &names);
            for key in &favourites {
                if !nodes.iter().any(|n| n["key"] == key.as_str()) {
                    let name = names.get(key).cloned().unwrap_or_default();
                    nodes.push(serde_json::json!({"key": key, "name": name, "type": 0, "hops": 0, "snr": 0.0, "rssi": 0.0, "seen": 0, "count": 0}));
                }
            }
            for entry in nodes.iter_mut() {
                let favourite = entry["key"].as_str().is_some_and(|key| favourites.contains(key));
                entry["favorite"] = favourite.into();
            }
            nodes.sort_by(|a, b| {
                b["favorite"].as_bool().cmp(&a["favorite"].as_bool()).then(b["seen"].as_u64().cmp(&a["seen"].as_u64()))
            });
            reply["nodes"] = serde_json::json!(nodes);
            let queued = node.join("scan").exists();
            let (active, found) = scan_state(&node);
            reply["scanning"] = queued.into();
            // waiting: asked for, the radio has not started; active: listening now; found: nodes that answered.
            reply["scan"] = serde_json::json!({"waiting": queued && !active, "active": queued && active, "found": found});
        }
        operation if operation.starts_with("set-name:") => {
            let name = unhex(&operation["set-name:".len()..]).unwrap_or_default();
            let pending = node.join("name.tmp");
            fs::write(&pending, &name)?;
            fs::rename(pending, node.join("name.txt"))?;
            reply["queued"] = "set-name".into();
        }
        operation if operation.starts_with("favorite:") => {
            let mut fields = operation.split(':').skip(1);
            let key = fields.next().unwrap_or_default().to_ascii_lowercase();
            let _guard = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
            let mut set = favorites(&node)?;
            if fields.next() == Some("1") {
                if set.len() >= 200 && !set.contains(&key) {
                    reply["error"] = "Too many favourites.".into();
                    return Ok(reply);
                }
                set.insert(key.clone());
            } else {
                set.remove(&key);
            }
            let pending = node.join("favorites.tmp");
            fs::write(&pending, set.iter().map(|key| format!("{key}\n")).collect::<String>())?;
            fs::rename(pending, node.join("favorites.txt"))?;
            reply["favorites"] = serde_json::json!(set);
        }
        "channel-list" => {
            reply["channels"] = serde_json::json!(channels(&node)?.into_keys().collect::<Vec<_>>());
        }
        operation
            if operation.starts_with("channel-join:")
                || operation.starts_with("channel-leave:") =>
        {
            use sha2::{Digest, Sha256};
            let _guard = GRANTS.lock().unwrap_or_else(|error| error.into_inner());
            let mut configured = channels(&node)?;
            let mut fields = operation.split(':');
            let verb = fields.next().unwrap();
            let name = fields.next().unwrap();
            if verb == "channel-join" {
                if configured.len() >= 7 && !configured.contains_key(name) {
                    reply["error"] = "All seven channel slots are in use.".into();
                    return Ok(reply);
                }
                let key = fields
                    .next()
                    .filter(|key| !key.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        Sha256::digest(format!("#{name}"))[..16]
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect()
                    });
                if configured
                    .get(name)
                    .is_some_and(|existing| !existing.eq_ignore_ascii_case(&key))
                    || configured.iter().any(|(existing, secret)| {
                        existing != name && secret.eq_ignore_ascii_case(&key)
                    })
                {
                    reply["error"] = "That name or channel key is already in use.".into();
                    return Ok(reply);
                }
                configured.insert(name.into(), key);
            } else {
                configured.remove(name);
            }
            let pending = node.join("channels.tmp");
            let records: String = configured
                .iter()
                .map(|(name, key)| format!("{name} {key}\n"))
                .collect();
            fs::write(&pending, records)?;
            fs::rename(pending, node.join("channels.txt"))?;
            reply["channels"] = serde_json::json!(configured.into_keys().collect::<Vec<_>>());
        }
        "direct-list" => {
            let conversations = direct_conversations(&node)?;
            reply["contacts"] = serde_json::json!(conversations);
        }
        operation
            if operation == "public-history"
                || operation.starts_with("direct-history:")
                || operation.starts_with("channel-history:") =>
        {
            let name = conversation_name(operation);
            let (messages, unread, last_id) = conversation_history(&node, &name)?;
            if let Some(contact) = name.strip_prefix("direct-") {
                reply["contact"] = contact.into();
                if let Ok(error) = fs::read_to_string(node.join(format!("{name}-error.txt"))) {
                    reply["error"] = error.into();
                }
            }
            if let Some(channel) = name.strip_prefix("channel-") {
                reply["channel"] = channel.into();
            }
            reply["messages"] = serde_json::json!(messages);
            reply["unread"] = unread.into();
            reply["lastId"] = last_id.into();
            reply["totalUnread"] = total_unread(&node, &name, if name.starts_with("channel-") && !channels(&node)?.contains_key(&name[8..]) { 0 } else { unread })?.into();
        }
        operation
            if operation.starts_with("public-send:")
                || operation.starts_with("direct-send:")
                || operation.starts_with("channel-send:") =>
        {
            let _guard = GRANTS.lock().unwrap_or_else(|err| err.into_inner());
            let name = conversation_name(operation);
            let request = node.join(if name == "public" {
                "public-send.txt"
            } else {
                if name.starts_with("channel-") {
                    "channel-send.txt"
                } else {
                    "direct-send.txt"
                }
            });
            if request.exists() {
                reply["error"] = "A message is already queued. Try again shortly.".into();
            } else {
                let message = unhex(operation.rsplit(':').next().unwrap_or(""))
                    .ok_or(io::ErrorKind::InvalidInput)?;
                let pending = request.with_extension("tmp");
                if let Some(contact) = name.strip_prefix("direct-") {
                    let mut record = contact.as_bytes().to_vec();
                    record.extend(message);
                    fs::write(&pending, record)?;
                    reply["queued"] = "direct-send".into();
                } else if let Some(channel) = name.strip_prefix("channel-") {
                    if !channels(&node)?.contains_key(channel) {
                        reply["error"] = "Join the channel before sending.".into();
                        return Ok(reply);
                    }
                    let mut record = format!("{channel}\n").into_bytes();
                    record.extend(message);
                    fs::write(&pending, record)?;
                    reply["queued"] = "channel-send".into();
                } else {
                    fs::write(&pending, message)?;
                    reply["queued"] = "public-send".into();
                }
                fs::rename(pending, request)?;
            }
        }
        operation
            if operation.starts_with("public-read:")
                || operation.starts_with("direct-read:")
                || operation.starts_with("channel-read:") =>
        {
            let _guard = GRANTS.lock().unwrap_or_else(|err| err.into_inner());
            let requested = operation
                .rsplit(':')
                .next()
                .unwrap_or("")
                .parse::<u64>()
                .map_err(io::Error::other)?;
            let name = conversation_name(operation);
            let (_, _, last_id) = conversation_history(&node, &name)?;
            let read = node.join(format!("{name}-read.txt"));
            let previous = fs::read_to_string(&read)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let pending = read.with_extension("tmp");
            fs::write(&pending, previous.max(requested.min(last_id)).to_string())?;
            fs::rename(pending, read)?;
            let (_, unread, _) = conversation_history(&node, &name)?;
            if let Some(contact) = name.strip_prefix("direct-") {
                reply["contact"] = contact.into();
            }
            if let Some(channel) = name.strip_prefix("channel-") {
                reply["channel"] = channel.into();
            }
            reply["unread"] = unread.into();
            reply["totalUnread"] = total_unread(&node, &name, if name.starts_with("channel-") && !channels(&node)?.contains_key(&name[8..]) { 0 } else { unread })?.into();
        }
        _ => unreachable!("operation was verified"),
    }
    Ok(reply)
}

/// The radio worker's scan progress (scan.txt: "<1 while scanning> <nodes answered>").
fn scan_state(node: &Path) -> (bool, u64) {
    let text = fs::read_to_string(node.join("scan.txt")).unwrap_or_default();
    let mut fields = text.split_whitespace();
    let active = fields.next() == Some("1");
    let found = fields.next().and_then(|found| found.parse().ok()).unwrap_or(0);
    (active, found)
}

/// The names the radio worker knows, by public key: heard nodes and contacts.
fn node_names(node: &Path) -> std::collections::HashMap<String, String> {
    fs::read_to_string(node.join("names.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (key, name) = line.split_once(' ')?;
            direct_key(key).then_some(())?;
            Some((key.to_ascii_lowercase(), clean_name(&unhex(name)?)))
        })
        .filter(|(_, name)| !name.is_empty())
        .collect()
}

fn clean_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().filter(|c| !c.is_control()).take(32).collect()
}

/// The nodes the radio worker has heard (heard.txt, written by meshcore-host).
fn heard_nodes(node: &Path, names: &std::collections::HashMap<String, String>) -> Vec<serde_json::Value> {
    fs::read_to_string(node.join("heard.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            if fields.len() < 7 || !direct_key(fields[0]) {
                return None;
            }
            let key = fields[0].to_ascii_lowercase();
            let name = fields.get(7).and_then(|hex| unhex(hex)).map(|bytes| clean_name(&bytes)).filter(|name| !name.is_empty())
                .or_else(|| names.get(&key).cloned())
                .unwrap_or_default();
            Some(serde_json::json!({
                "key": key,
                "name": name,
                "type": fields[1].parse::<u8>().ok()?,
                "hops": fields[2].parse::<u8>().ok()?,
                "snr": fields[3].parse::<f64>().ok()?,
                "rssi": fields[4].parse::<f64>().ok()?,
                "seen": fields[5].parse::<u64>().ok()?,
                "count": fields[6].parse::<u64>().ok()?,
            }))
        })
        .collect()
}

fn favorites(node: &Path) -> io::Result<std::collections::BTreeSet<String>> {
    match fs::read_to_string(node.join("favorites.txt")) {
        Ok(text) => Ok(text.lines().filter(|key| direct_key(key)).map(str::to_ascii_lowercase).collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(error),
    }
}

/// One row of the conversation list: who, the latest message, how many are unread.
fn conversation_summary(node: &Path, name: &str, kind: &str, id: &str, title: &str) -> io::Result<serde_json::Value> {
    let (recent, unread, _) = conversation_history(node, name)?;
    let last = recent.back();
    Ok(serde_json::json!({
        "kind": kind,
        "id": id,
        "title": title,
        "unread": unread,
        "last": last.and_then(|message| message["text"].as_str()).unwrap_or(""),
        "time": last.and_then(|message| message["timestamp"].as_u64()).unwrap_or(0),
        "mine": last.and_then(|message| message["mine"].as_bool()).unwrap_or(false),
    }))
}

fn direct_conversations(node: &Path) -> io::Result<Vec<String>> {
    let mut conversations = Vec::new();
    for entry in fs::read_dir(node)? {
        let name = entry?.file_name();
        let Some(key) = name
            .to_str()
            .and_then(|name| name.strip_prefix("direct-"))
            .and_then(|name| name.strip_suffix("-history.txt"))
        else {
            continue;
        };
        if direct_key(key) {
            conversations.push(key.to_owned());
        }
    }
    conversations.sort_unstable();
    Ok(conversations)
}

fn unread_only(node: &Path, name: &str) -> io::Result<u64> {
    use std::io::BufRead;
    let last_read = match fs::read_to_string(node.join(format!("{name}-read.txt"))) {
        Ok(value) => value.parse::<usize>().unwrap_or(0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let log = match fs::File::open(node.join(format!("{name}-history.txt"))) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut log = std::io::BufReader::new(log);
    let mut record = String::new();
    let mut unread = 0;
    let mut index = 0;
    while log.read_line(&mut record)? != 0 {
        index += 1;
        if index > last_read && let Some(payload) = record.trim_end_matches(['\r', '\n']).strip_prefix("in ")
            && let Some((timestamp, body)) = payload.split_once(' ')
            && timestamp.parse::<u32>().is_ok()
            && unhex(body).is_some_and(|bytes| std::str::from_utf8(&bytes).is_ok()) {
            unread += 1;
        }
        record.clear();
    }
    Ok(unread)
}

fn total_unread(node: &Path, current: &str, current_unread: u64) -> io::Result<u64> {
    let mut unread = current_unread;
    if current != "public" {
        unread = unread.saturating_add(unread_only(node, "public")?);
    }
    for contact in direct_conversations(node)? {
        let name = format!("direct-{contact}");
        if name != current { unread = unread.saturating_add(unread_only(node, &name)?); }
    }
    for name in channels(node)?.into_keys() {
        let name = format!("channel-{name}");
        if name != current { unread = unread.saturating_add(unread_only(node, &name)?); }
    }
    Ok(unread)
}

fn conversation_name(operation: &str) -> String {
    if operation.starts_with("direct-") {
        let key = operation
            .split(':')
            .nth(1)
            .expect("validated direct operation");
        format!("direct-{}", key.to_ascii_lowercase())
    } else if operation.starts_with("channel-") {
        format!(
            "channel-{}",
            operation
                .split(':')
                .nth(1)
                .expect("validated channel operation")
        )
    } else {
        "public".into()
    }
}

fn provision(dir: &Path, person: &str, create: bool) -> io::Result<Option<String>> {
    let root = dir.join("meshcore-nodes");
    let node = root.join(person);
    let key_path = node.join("public-key.txt");
    let read_key = || match fs::read_to_string(&key_path) {
        Ok(key) if key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            Ok(Some(key))
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid MeshCore public key",
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    };
    let key = read_key()?;
    if key.is_some() || !create {
        return Ok(key);
    }
    fs::create_dir_all(&node)?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(node.join("provision"))?;
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(key) = read_key()? {
            return Ok(Some(key));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "MeshCore worker did not provision the identity",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maarse_crypto::{
        Identity,
        meshcore_access::{sign_request, verify_request},
        person::{DeviceCert, DeviceEntry, DeviceList},
    };
    use rand_core::{OsRng, RngCore};

    #[test]
    fn overview_nodes_and_favourites_come_from_the_worker_files() {
        let dir = std::env::temp_dir().join(format!("maarse-mc-overview-{}", std::process::id()));
        let person = "0123456789abcdef0123456789abcdef";
        let node = dir.join("meshcore-nodes").join(person);
        fs::create_dir_all(&node).unwrap();
        fs::write(node.join("public-key.txt"), "ab".repeat(32)).unwrap();
        let (mette, skagen) = ("11".repeat(32), "22".repeat(32));
        let hex = |text: &str| text.bytes().map(|byte| format!("{byte:02x}")).collect::<String>();
        fs::write(node.join("public-history.txt"), format!("in 1000 {}\nin 1001 {}\n", hex("hello"), hex("anyone?"))).unwrap();
        fs::write(node.join(format!("direct-{mette}-history.txt")), format!("in 900 {}\nout 950 {}\n", hex("hi"), hex("hey"))).unwrap();
        fs::write(node.join("channels.txt"), format!("hike {}\n", "00".repeat(16))).unwrap();
        fs::write(node.join("names.txt"), format!("{mette} {}\n", hex("Mette"))).unwrap();
        fs::write(
            node.join("heard.txt"),
            format!("{mette} 1 3 9.5 -87 2000 4 {}\n{skagen} 2 1 12.0 -71 2100 9 {}\n", hex("Mette"), hex("Skagen")),
        )
        .unwrap();

        // The conversation list: Public first, then the latest message first.
        let overview = radio_operation(&dir, person, "overview").unwrap();
        let rows = overview["conversations"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0]["id"].as_str(), rows[0]["unread"].as_u64(), rows[0]["last"].as_str()), (Some("public"), Some(2), Some("anyone?")));
        assert_eq!((rows[1]["kind"].as_str(), rows[1]["title"].as_str(), rows[1]["last"].as_str()), (Some("direct"), Some("Mette"), Some("hey")));
        assert_eq!((rows[1]["unread"].as_u64(), rows[1]["mine"].as_bool()), (Some(1), Some(true)));
        assert_eq!((rows[2]["kind"].as_str(), rows[2]["title"].as_str()), (Some("channel"), Some("#hike")));
        assert_eq!(overview["totalUnread"], 3);

        // Heard nodes, newest first; a favourite goes to the top.
        let nodes = radio_operation(&dir, person, "nodes").unwrap();
        let list = nodes["nodes"].as_array().unwrap();
        assert_eq!((list[0]["name"].as_str(), list[0]["type"].as_u64(), list[0]["hops"].as_u64()), (Some("Skagen"), Some(2), Some(1)));
        assert_eq!(list[0]["favorite"], false);
        let marked = radio_operation(&dir, person, &format!("favorite:{mette}:1")).unwrap();
        assert_eq!(marked["favorites"], serde_json::json!([mette]));
        let nodes = radio_operation(&dir, person, "nodes").unwrap();
        let list = nodes["nodes"].as_array().unwrap();
        assert_eq!((list[0]["name"].as_str(), list[0]["favorite"].as_bool()), (Some("Mette"), Some(true)));
        radio_operation(&dir, person, &format!("favorite:{mette}:0")).unwrap();
        assert_eq!(radio_operation(&dir, person, "nodes").unwrap()["nodes"][0]["name"], "Skagen");

        // What happened to a sent message: repeaters that passed it on, or the acknowledgement.
        fs::write(node.join(format!("direct-{mette}-heard.txt")), "950 2 6.5 1
").unwrap();
        let history = radio_operation(&dir, person, &format!("direct-history:{mette}")).unwrap();
        let sent = &history["messages"][1];
        assert_eq!((sent["mine"].as_bool(), sent["repeats"].as_u64(), sent["delivered"].as_bool(), sent["snr"].as_f64()), (Some(true), Some(2), Some(true), Some(6.5)));
        assert!(history["messages"][0].get("repeats").is_none());

        // Scan progress: asked for, then listening with a count, then finished.
        fs::write(node.join("scan"), "").unwrap();
        let waiting = radio_operation(&dir, person, "nodes").unwrap();
        assert_eq!((waiting["scan"]["waiting"].as_bool(), waiting["scan"]["active"].as_bool()), (Some(true), Some(false)));
        fs::write(node.join("scan.txt"), "1 3\n").unwrap();
        let running = radio_operation(&dir, person, "nodes").unwrap();
        assert_eq!((running["scan"]["active"].as_bool(), running["scan"]["found"].as_u64()), (Some(true), Some(3)));
        fs::remove_file(node.join("scan")).unwrap();
        fs::write(node.join("scan.txt"), "0 5\n").unwrap();
        let done = radio_operation(&dir, person, "nodes").unwrap();
        assert_eq!((done["scan"]["active"].as_bool(), done["scan"]["found"].as_u64()), (Some(false), Some(5)));

        // Only well-formed requests are accepted.
        assert!(valid_operation("overview") && valid_operation("nodes"));
        assert!(valid_operation(&format!("favorite:{mette}:1")));
        assert!(!valid_operation(&format!("favorite:{mette}:2")) && !valid_operation("favorite:short:1"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn access_requires_grant_and_rejects_older_device_lists() {
        let path =
            std::env::temp_dir().join(format!("maarse-meshcore-access-{}", OsRng.next_u64()));
        fs::create_dir(&path).unwrap();
        let master = Identity::generate(&mut OsRng);
        let device = Identity::generate(&mut OsRng);
        let person = master.address();
        let certificate = DeviceCert::issue(&master, &device.address(), 0).to_bytes();
        let request = b"access";
        let challenge = [1; 32];
        let signature = sign_request(&device, &challenge, request, &person);
        let proof_for = |version| {
            let list = DeviceList::issue(
                &master,
                version,
                &[DeviceEntry {
                    addr: device.address(),
                    revoked: false,
                }],
            )
            .unwrap();
            verify_request(
                &device.public(),
                &certificate,
                list.to_bytes().as_slice(),
                &challenge,
                request,
                &signature,
            )
            .unwrap()
        };
        let old = proof_for(1);
        let new = proof_for(2);
        assert!(authorize_device(&path, &old).is_err());
        set_grant(&path, &person.to_string(), true).unwrap();
        authorize_device(&path, &old).unwrap();
        authorize_device(&path, &new).unwrap();
        authorize_device(&path, &new).unwrap();
        assert!(authorize_device(&path, &old).is_err());
        set_grant(&path, &person.to_string(), false).unwrap();
        assert!(authorize_device(&path, &new).is_err());
        fs::remove_dir_all(path).unwrap();
    }
}
