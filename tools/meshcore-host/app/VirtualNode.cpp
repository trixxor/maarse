#include "VirtualNode.h"
#include "portable.h"
#include <ctime>
#include <cstdio>
#include <cerrno>
#include <algorithm>
#include <cctype>
#include <fstream>
#include <sstream>

#define SEND_TIMEOUT_BASE_MILLIS          500
#define FLOOD_SEND_TIMEOUT_FACTOR         16.0f
#define DIRECT_SEND_PERHOP_FACTOR         6.0f
#define DIRECT_SEND_PERHOP_EXTRA_MILLIS   250

#define PUBLIC_GROUP_PSK  "izOH6cXN6mrJ5e26oRXNcg=="

uint32_t SystemRTC::getCurrentTime() { return (uint32_t)time(nullptr); }

void SystemRNG::random(uint8_t* dest, size_t sz) {
  if (!portable::randomBytes(dest, sz)) {
    std::abort();
  }
}

static const char* typeName(uint8_t type) {
  if (type == ADV_TYPE_CHAT) return "Chat";
  if (type == ADV_TYPE_REPEATER) return "Repeater";
  if (type == ADV_TYPE_ROOM) return "Room";
  if (type == ADV_TYPE_SENSOR) return "Sensor";
  return "??";
}

static std::string ts() {
  time_t t = time(nullptr); char b[16];
  strftime(b, sizeof(b), "%H:%M:%S", localtime(&t));
  return b;
}

// ------------------------------------------------------------ persistence
// same on-disk record layout as simple_secure_chat's /contacts file

void VirtualNode::loadContacts() {
  FILE* f = fopen(path("contacts").c_str(), "rb");
  if (!f) return;
  for (;;) {
    ContactInfo c; memset(&c, 0, sizeof(c));
    uint8_t pub_key[32], unused; uint32_t reserved;
    bool ok = fread(pub_key, 1, 32, f) == 32;
    ok = ok && fread(c.name, 1, 32, f) == 32;
    ok = ok && fread(&c.type, 1, 1, f) == 1;
    ok = ok && fread(&c.flags, 1, 1, f) == 1;
    ok = ok && fread(&unused, 1, 1, f) == 1;
    ok = ok && fread(&reserved, 1, 4, f) == 4;
    ok = ok && fread(&c.out_path_len, 1, 1, f) == 1;
    ok = ok && fread(&c.last_advert_timestamp, 1, 4, f) == 4;
    ok = ok && fread(c.out_path, 1, 64, f) == 64;
    if (!ok) break;
    c.id = mesh::Identity(pub_key);
    if (!addContact(c)) break;
  }
  fclose(f);
}

void VirtualNode::saveContacts() {
  FILE* f = fopen(path("contacts").c_str(), "wb");
  if (!f) return;
  ContactsIterator iter = startContactsIterator();
  ContactInfo c; uint8_t unused = 0; uint32_t reserved = 0;
  while (iter.hasNext(this, c)) {
    fwrite(c.id.pub_key, 1, 32, f);
    fwrite(c.name, 1, 32, f);
    fwrite(&c.type, 1, 1, f);
    fwrite(&c.flags, 1, 1, f);
    fwrite(&unused, 1, 1, f);
    fwrite(&reserved, 1, 4, f);
    fwrite(&c.out_path_len, 1, 1, f);
    fwrite(&c.last_advert_timestamp, 1, 4, f);
    fwrite(c.out_path, 1, 64, f);
  }
  fclose(f);
}

void VirtualNode::savePrefs() {
  FILE* f = fopen(path("node_prefs").c_str(), "wb");
  if (f) { fwrite(&_prefs, sizeof(_prefs), 1, f); fclose(f); }
}

bool VirtualNode::begin(const std::string& data_dir, const char* default_name, bool service) {
  _dir = data_dir;
  _service = service;
  if (!portable::makeDirectory(_dir.c_str())) return false;
  BaseChatMesh::begin();

  // identity: stored with the official LocalIdentity serialiser
  bool have_id = false;
  if (FILE* f = fopen(path("identity").c_str(), "rb")) {
    uint8_t buf[128]; size_t n = fread(buf, 1, sizeof(buf), f); fclose(f);
    if (n != PRV_KEY_SIZE + PUB_KEY_SIZE) return false;
    self_id.readFrom(buf, n);
    have_id = true;
  } else if (errno != ENOENT) {
    return false;
  }
  if (!have_id) {
    self_id = mesh::LocalIdentity(getRNG());
    int count = 0;
    while (count < 10 && (self_id.pub_key[0] == 0x00 || self_id.pub_key[0] == 0xFF)) {  // reserved id hashes
      self_id = mesh::LocalIdentity(getRNG()); count++;
    }
    uint8_t buf[128]; size_t n = self_id.writeTo(buf, sizeof(buf));
    const auto temporary = path(("identity.new." + std::to_string(portable::processId())).c_str());
    FILE* f = fopen(temporary.c_str(), "wb");
    if (!f) return false;
    const bool saved = n == PRV_KEY_SIZE + PUB_KEY_SIZE && fwrite(buf, 1, n, f) == n
      && fflush(f) == 0 && portable::syncFile(f);
    const bool closed = fclose(f) == 0;
    if (!saved || !closed || !portable::replaceFile(temporary.c_str(), path("identity").c_str())) {
      remove(temporary.c_str());
      return false;
    }
  }

  if (FILE* f = fopen(path("node_prefs").c_str(), "rb")) { fread(&_prefs, sizeof(_prefs), 1, f); fclose(f); }
  if (!_prefs.node_name[0]) {
    StrHelper::strncpy(_prefs.node_name, default_name, sizeof(_prefs.node_name));
    if (_prefs.airtime_factor <= 0) _prefs.airtime_factor = 1.0;
    savePrefs();
  }

  loadContacts();
  _public = addChannel("Public", PUBLIC_GROUP_PSK);
  return true;
}

// ------------------------------------------------------------ mesh callbacks

void VirtualNode::onDiscoveredContact(ContactInfo& contact, bool is_new, uint8_t path_len, const uint8_t* path) {
  printf("%s [%s] ADVERT %s: %s (%s), hops=%d\n", ts().c_str(), tag(), is_new ? "new" : "upd",
         contact.name, typeName(contact.type), path_len & 63);
  saveContacts();
}

void VirtualNode::onContactPathUpdated(const ContactInfo& contact) {
  printf("%s [%s] PATH to %s, path_len=%d\n", ts().c_str(), tag(), contact.name, (int)contact.out_path_len);
  saveContacts();
}

ContactInfo* VirtualNode::processAck(const uint8_t* data) {
  for (auto& sent : _sent) {
    if (sent.ack && !sent.delivered && memcmp(data, &sent.ack, 4) == 0) { sent.delivered = true; _sentDirty = true; }
  }
  if (expected_ack_crc && memcmp(data, &expected_ack_crc, 4) == 0) {
    printf("%s [%s] Got ACK! (round trip: %lu ms)\n", ts().c_str(), tag(), _ms->getMillis() - last_msg_sent);
    expected_ack_crc = 0;
  }
  return NULL;
}

void VirtualNode::onMessageRecv(const ContactInfo& from, mesh::Packet* pkt, uint32_t sender_timestamp, const char* text) {
  printf("%s [%s] MSG (%s) from %s: %s\n", ts().c_str(), tag(), pkt->isRouteDirect() ? "DIRECT" : "FLOOD", from.name, text);
  fflush(stdout);
  char hex[PUB_KEY_SIZE * 2 + 1];
  mesh::Utils::toHex(hex, from.id.pub_key, PUB_KEY_SIZE);
  std::string key(hex);
  std::transform(key.begin(), key.end(), key.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
  logMessage("direct-" + key, false, sender_timestamp, text);
}

void VirtualNode::onChannelMessageRecv(const mesh::GroupChannel& channel, mesh::Packet* pkt, uint32_t timestamp, const char* text) {
  printf("%s [%s] PUBLIC (%s, hops %d): %s\n", ts().c_str(), tag(), pkt->isRouteDirect() ? "direct" : "flood",
         pkt->getPathHashCount(), text);
  fflush(stdout);
  if (_public && memcmp(channel.secret, _public->channel.secret, sizeof(channel.secret)) == 0) {
    logMessage("public", false, timestamp, text);
    return;
  }
  for (int index = 1; index < MAX_GROUP_CHANNELS; ++index) {
    ChannelDetails details{};
    if (getChannel(index, details) && details.name[0] && memcmp(channel.secret, details.channel.secret, sizeof(channel.secret)) == 0) {
      logMessage(std::string("channel-") + details.name, false, timestamp, text);
      return;
    }
  }
}

void VirtualNode::logMessage(const std::string& conversation, bool outgoing, uint32_t timestamp, const char* message) {
  if (!_service) return;
  const size_t length = strnlen(message, MAX_TEXT_LEN);
  char encoded[MAX_TEXT_LEN * 2 + 1];
  mesh::Utils::toHex(encoded, reinterpret_cast<const uint8_t*>(message), length);
  if (FILE* out = fopen(path((conversation + "-history.txt").c_str()).c_str(), "ab")) {
    fprintf(out, "%s %u %s\n", outgoing ? "out" : "in", timestamp, encoded);
    fclose(out);
  }
}

void VirtualNode::reloadChannels() {
  std::ifstream file(path("channels.txt"), std::ios::binary);
  const std::string config((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
  if (config == _channelConfig) return;
  _channelConfig = config;
  std::istringstream records(config);
  int index = 1;
  std::string name, key;
  while (index < MAX_GROUP_CHANNELS && records >> name >> key) {
    ChannelDetails details{};
    if (name.size() > 24 || !mesh::Utils::fromHex(details.channel.secret, 16, key.c_str())) continue;
    StrHelper::strncpy(details.name, name.c_str(), sizeof(details.name));
    setChannel(index++, details);
  }
  for (; index < MAX_GROUP_CHANNELS; ++index) setChannel(index, ChannelDetails{});
}

bool VirtualNode::sendNamedChannel(const std::string& name, const char* message) {
  for (int index = 1; index < MAX_GROUP_CHANNELS; ++index) {
    ChannelDetails details{};
    if (getChannel(index, details) && name == details.name) return sendChannelMessage(details, "channel-" + name, message);
  }
  return false;
}

bool VirtualNode::sendPublic(const char* message) {
  return _public && sendChannelMessage(*_public, "public", message);
}

bool VirtualNode::sendChannelMessage(const ChannelDetails& details, const std::string& conversation, const char* message) {
  uint8_t temp[5 + MAX_TEXT_LEN + 1];
  const uint32_t timestamp = getRTCClock()->getCurrentTime();
  memcpy(temp, &timestamp, 4);
  temp[4] = 0;
  snprintf(reinterpret_cast<char*>(&temp[5]), MAX_TEXT_LEN + 1, "%s: %s", _prefs.node_name, message);
  const size_t length = strlen(reinterpret_cast<const char*>(&temp[5]));
  auto packet = createGroupDatagram(PAYLOAD_TYPE_GRP_TXT, details.channel, temp, 5 + length);
  if (!packet) return false;
  Sent sent;
  sent.conversation = conversation;
  sent.timestamp = timestamp;
  packet->calculatePacketHash(sent.hash);
  sent.tracksHash = true;
  sendFlood(packet);
  trackSent(sent);
  logMessage(conversation, true, timestamp, message);
  return true;
}

void VirtualNode::sendDirect(const std::string& contact, const char* message) {
  uint8_t key[PUB_KEY_SIZE];
  if (contact.size() != 2 * PUB_KEY_SIZE || !mesh::Utils::fromHex(key, PUB_KEY_SIZE, contact.c_str())) return;
  const auto conversation = "direct-" + contact;
  const auto errorFile = path((conversation + "-error.txt").c_str());
  const auto fail = [&](const char* error) {
    if (FILE* file = fopen(errorFile.c_str(), "wb")) { fputs(error, file); fclose(file); }
  };
  const ContactInfo* recipient = lookupContactByPubKey(key, PUB_KEY_SIZE);
  if (!recipient) {
    fail("Contact is unknown. Ask the contact to announce before sending.");
    return;
  }
  const uint32_t timestamp = getRTCClock()->getCurrentTime();
  uint32_t ack, timeout;
  if (sendMessage(*recipient, timestamp, 0, message, ack, timeout) == MSG_SEND_FAILED) {
    fail("Could not queue the direct message on the radio.");
    return;
  }
  remove(errorFile.c_str());
  Sent sent;
  sent.conversation = conversation;
  sent.timestamp = timestamp;
  sent.ack = ack;
  trackSent(sent);
  logMessage(conversation, true, timestamp, message);
}

void VirtualNode::trackSent(const Sent& sent) {
  if (!_service) return;
  if (_sent.size() >= 40) _sent.erase(_sent.begin());
  _sent.push_back(sent);
  _sentDirty = true;
}

// A repeater passing on our own channel message sends it again with its id on the path:
// every different repeater heard counts once.
void VirtualNode::logRx(mesh::Packet* packet, int, float) {
  if (!_service || _sent.empty()) return;
  uint8_t hash[MAX_HASH_SIZE];
  packet->calculatePacketHash(hash);
  const uint8_t count = packet->getPathHashCount();
  if (count == 0) return;
  const uint8_t repeater = packet->path[(count - 1) * packet->getPathHashSize()];
  for (auto& sent : _sent) {
    if (!sent.tracksHash || memcmp(sent.hash, hash, MAX_HASH_SIZE) != 0) continue;
    bool known = false;
    for (unsigned i = 0; i < sent.seenCount; ++i) known = known || sent.seen[i] == repeater;
    if (known) return;
    if (sent.seenCount < sizeof(sent.seen)) sent.seen[sent.seenCount++] = repeater;
    sent.repeats++;
    if (packet->getSNR() > sent.snr) sent.snr = packet->getSNR();
    _sentDirty = true;
    return;
  }
}

void VirtualNode::publishSent() {
  std::vector<std::string> done;
  for (const auto& first : _sent) {
    if (std::find(done.begin(), done.end(), first.conversation) != done.end()) continue;
    done.push_back(first.conversation);
    const auto temporary = path((first.conversation + "-heard.new").c_str());
    FILE* output = fopen(temporary.c_str(), "wb");
    if (!output) continue;
    for (const auto& sent : _sent) {
      if (sent.conversation != first.conversation) continue;
      fprintf(output, "%u %u %.1f %d\n", sent.timestamp, sent.repeats, sent.snr, sent.delivered ? 1 : 0);
    }
    if (fclose(output) == 0) portable::replaceFile(temporary.c_str(), path((first.conversation + "-heard.txt").c_str()).c_str());
  }
  _sentDirty = false;
  _sentPublishedAt = millis();
}

static std::string contactKey(const ContactInfo& contact) {
  char hex[PUB_KEY_SIZE * 2 + 1];
  mesh::Utils::toHex(hex, contact.id.pub_key, PUB_KEY_SIZE);
  std::string key(hex);
  std::transform(key.begin(), key.end(), key.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
  return key;
}

void VirtualNode::publishRepeater(const std::string& key, const char* text) {
  const auto name = path(("repeater-" + key + ".txt").c_str());
  const auto temporary = name + ".new";
  if (FILE* file = fopen(temporary.c_str(), "wb")) {
    fwrite(text, 1, strnlen(text, MAX_TEXT_LEN), file);
    if (fclose(file) == 0) portable::replaceFile(temporary.c_str(), name.c_str());
  }
}

void VirtualNode::repeaterRequest(bool login, const std::string& key, const char* value) {
  uint8_t bytes[PUB_KEY_SIZE];
  if (key.size() != 2 * PUB_KEY_SIZE || !std::all_of(key.begin(), key.end(), [](unsigned char c) { return std::isxdigit(c); }) ||
      !mesh::Utils::fromHex(bytes, PUB_KEY_SIZE, key.c_str())) return;
  const ContactInfo* contact = lookupContactByPubKey(bytes, PUB_KEY_SIZE);
  if (!contact || contact->type != ADV_TYPE_REPEATER) {
    publishRepeater(key, "Repeater is unknown. Receive its advert before connecting.");
    return;
  }
  uint32_t timeout = 0;
  if (login) {
    if (!_repeaterKey.empty() && _repeaterKey != key) publishRepeater(_repeaterKey, "Login required.");
    _repeaterKey = key;
    _repeaterAdmin = false;
    _repeaterPending = sendLogin(*contact, value, timeout) != MSG_SEND_FAILED;
    _repeaterDeadline = millis() + timeout;
    publishRepeater(key, _repeaterPending ? "Waiting for repeater login..." : "Could not send login.");
  } else if (!_repeaterAdmin || _repeaterKey != key || static_cast<long>(millis() - _repeaterDeadline) >= 0) {
    publishRepeater(key, "Administrator login required.");
  } else {
    const auto timestamp = getRTCClock()->getCurrentTimeUnique();
    const auto result = sendCommandData(*contact, timestamp, 0, TXT_TYPE_CLI_DATA, value, timeout);
    publishRepeater(key, result == MSG_SEND_FAILED ? "Could not send command." : "Waiting for repeater response...");
  }
}

void VirtualNode::onContactResponse(const ContactInfo& contact, const uint8_t* data, uint8_t len) {
  if (!_repeaterPending || contactKey(contact) != _repeaterKey) return;
  _repeaterPending = false;
  _repeaterAdmin = len >= 7 && data[4] == 0 && (data[6] & 1) != 0;
  _repeaterDeadline = millis() + 600000;
  publishRepeater(_repeaterKey, _repeaterAdmin ? "Administrator login accepted." : "Repeater did not grant administrator access.");
}

void VirtualNode::onCommandDataRecv(const ContactInfo& contact, mesh::Packet*, uint32_t, const char* text) {
  if (_repeaterAdmin && contactKey(contact) == _repeaterKey) publishRepeater(_repeaterKey, text);
}

uint32_t VirtualNode::calcFloodTimeoutMillisFor(uint32_t pkt_airtime_millis) const {
  return SEND_TIMEOUT_BASE_MILLIS + (FLOOD_SEND_TIMEOUT_FACTOR * pkt_airtime_millis);
}

uint32_t VirtualNode::calcDirectTimeoutMillisFor(uint32_t pkt_airtime_millis, uint8_t path_len) const {
  uint8_t path_hash_count = path_len & 63;
  return SEND_TIMEOUT_BASE_MILLIS +
         ((pkt_airtime_millis * DIRECT_SEND_PERHOP_FACTOR + DIRECT_SEND_PERHOP_EXTRA_MILLIS) * (path_hash_count + 1));
}

void VirtualNode::onSendTimeout() {
  // simple_secure_chat's processAck() returns NULL (TODO upstream), so the base class
  // still fires the timeout after a successful ACK. Only report a real miss.
  if (expected_ack_crc == 0) return;
  expected_ack_crc = 0;
  printf("%s [%s] ERROR: timed out, no ACK.\n", ts().c_str(), tag());
}

// ------------------------------------------------------------ UI

void VirtualNode::showWelcome() {
  printf("[%s] node ready, public key: ", tag());
  mesh::Utils::printHex(Serial, self_id.pub_key, PUB_KEY_SIZE);
  printf("\n");
}

void VirtualNode::sendSelfAdvert(bool flood) {
  auto pkt = (_prefs.node_lat != 0 || _prefs.node_lon != 0)
                 ? createSelfAdvert(_prefs.node_name, _prefs.node_lat, _prefs.node_lon)
                 : createSelfAdvert(_prefs.node_name);
  if (!pkt) { printf("[%s] ERR: unable to create advert\n", tag()); return; }
  if (flood) sendFlood(pkt); else sendZeroHop(pkt);
  printf("[%s] advert sent (%s)\n", tag(), flood ? "flood" : "zero hop");
}

void VirtualNode::onContactVisit(const ContactInfo& contact) {
  if (contact.type == ADV_TYPE_NONE || !contact.name[0]) return;  // skip reserved anon slots
  char tmp[40];
  int32_t secs = contact.last_advert_timestamp - getRTCClock()->getCurrentTime();
  AdvertTimeHelper::formatRelativeTimeDiff(tmp, secs, false);
  printf("   %-24s %-8s %s\n", contact.name, typeName(contact.type), tmp);
}

void VirtualNode::importCard(const char* command) {
  while (*command == ' ') command++;
  if (memcmp(command, "meshcore://", 11) == 0) {
    command += 11;
    int len = (int)strlen(command);
    while (len > 0 && !mesh::Utils::isHexChar(command[len - 1])) len--;
    if (len % 2 == 0 && len / 2 <= (int)sizeof(tmp_buf) && mesh::Utils::fromHex(tmp_buf, len / 2, command)) {
      if (importContact(tmp_buf, len / 2)) { printf("   imported.\n"); saveContacts(); return; }
    }
  }
  printf("   error: invalid format\n");
}

void VirtualNode::handleCommand(const char* command) {
  while (*command == ' ') command++;

  if (memcmp(command, "send ", 5) == 0) {
    if (!curr_recipient) { printf("   ERROR: no recipient selected (use 'to' cmd).\n"); return; }
    uint32_t est_timeout;
    int result = sendMessage(*curr_recipient, getRTCClock()->getCurrentTime(), 0, &command[5], expected_ack_crc, est_timeout);
    if (result == MSG_SEND_FAILED) {
      printf("   ERROR: unable to send.\n");
    } else {
      last_msg_sent = _ms->getMillis();
      printf("   (message sent - %s, timeout %u ms)\n", result == MSG_SEND_SENT_FLOOD ? "FLOOD" : "DIRECT", est_timeout);
    }
  } else if (memcmp(command, "public ", 7) == 0) {
    puts(sendPublic(&command[7]) ? "   Sent." : "   ERROR: unable to send");
  } else if (memcmp(command, "list", 4) == 0) {
    int n = command[4] == ' ' ? atoi(&command[5]) : 0;
    printf("   %d contacts:\n", getNumContacts());
    scanRecentContacts(n, this);
  } else if (memcmp(command, "to ", 3) == 0) {
    curr_recipient = searchContactsByPrefix(&command[3]);
    if (curr_recipient) printf("   Recipient %s now selected.\n", curr_recipient->name);
    else printf("   Error: Name prefix not found.\n");
  } else if (strcmp(command, "to") == 0) {
    if (curr_recipient) printf("   Current: %s\n", curr_recipient->name);
    else printf("   Err: no recipient selected\n");
  } else if (strcmp(command, "advert") == 0) {
    sendSelfAdvert(false);
  } else if (strcmp(command, "advert flood") == 0) {
    sendSelfAdvert(true);
  } else if (strcmp(command, "reset path") == 0) {
    if (curr_recipient) { resetPathTo(*curr_recipient); saveContacts(); printf("   Done.\n"); }
  } else if (strcmp(command, "card") == 0) {
    auto pkt = createSelfAdvert(_prefs.node_name, _prefs.node_lat, _prefs.node_lon);
    if (pkt) {
      uint8_t len = pkt->writeTo(tmp_buf);
      releasePacket(pkt);
      mesh::Utils::toHex(hex_buf, tmp_buf, len);
      printf("meshcore://%s\n", hex_buf);
    }
  } else if (memcmp(command, "import ", 7) == 0) {
    importCard(&command[7]);
  } else if (memcmp(command, "set ", 4) == 0) {
    const char* config = &command[4];
    if (memcmp(config, "af ", 3) == 0) _prefs.airtime_factor = (float)atof(&config[3]);
    else if (memcmp(config, "name ", 5) == 0) StrHelper::strncpy(_prefs.node_name, &config[5], sizeof(_prefs.node_name));
    else if (memcmp(config, "lat ", 4) == 0) _prefs.node_lat = atof(&config[4]);
    else if (memcmp(config, "lon ", 4) == 0) _prefs.node_lon = atof(&config[4]);
    else { printf("  ERROR: unknown config: %s\n", config); return; }
    savePrefs();
    printf("  OK\n");
  } else if (strcmp(command, "scan") == 0 || memcmp(command, "scan ", 5) == 0) {
    int secs = command[4] == ' ' ? atoi(&command[5]) : 0;
    startScan(secs > 0 ? secs : 10, 0xFF);   // all node types (only repeaters answer in current firmware)
  } else if (strcmp(command, "heard") == 0) {
    printHeard();
  } else if (strcmp(command, "whoami") == 0) {
    showWelcome();
  } else {
    printf("   ERROR: unknown command: %s (try 'help')\n", command);
  }
}

// ------------------------------------------------------------ scan / heard
// Uses the official zero-hop node discovery (same packets as companion_radio's
// discovery screen and simple_repeater's responder):
//   REQ : CONTROL  [0x80|prefix_only] [filter] [tag:4] ([since:4])
//   RESP: CONTROL  [0x90|node_type] [snr_x4 as heard by responder] [tag:4] [pubkey:8|32]
#define CTL_TYPE_NODE_DISCOVER_REQ   0x80
#define CTL_TYPE_NODE_DISCOVER_RESP  0x90

std::string VirtualNode::nameFor(const uint8_t* pub_key) {
  for (auto& h : _heard) if (memcmp(h.pub_key, pub_key, PUB_KEY_SIZE) == 0 && h.name[0]) return h.name;
  ContactInfo* c = lookupContactByPubKey(pub_key, PUB_KEY_SIZE);
  return c ? c->name : "";
}

void VirtualNode::onAdvertRecv(mesh::Packet* packet, const mesh::Identity& id, uint32_t timestamp, const uint8_t* app_data, size_t app_data_len) {
  AdvertDataParser parser(app_data, (uint8_t)app_data_len);
  if (parser.isValid() && !id.matches(self_id)) {
    Heard* h = nullptr;
    for (auto& x : _heard) if (memcmp(x.pub_key, id.pub_key, PUB_KEY_SIZE) == 0) { h = &x; break; }
    if (!h) { _heard.push_back({}); h = &_heard.back(); memcpy(h->pub_key, id.pub_key, PUB_KEY_SIZE); }
    StrHelper::strncpy(h->name, parser.hasName() ? parser.getName() : "", sizeof(h->name));
    h->type = parser.getType();
    h->hops = packet->getPathHashCount();
    h->snr = packet->getSNR();
    h->rssi = _vradio.getLastRSSI();
    h->last_seen = getRTCClock()->getCurrentTime();
    h->count++;
    _heardChanged = true;
  }
  BaseChatMesh::onAdvertRecv(packet, id, timestamp, app_data, app_data_len);  // keep normal contact handling
}

void VirtualNode::onControlDataRecv(mesh::Packet* packet) {
  uint8_t type = packet->payload[0] & 0xF0;
  if (type != CTL_TYPE_NODE_DISCOVER_RESP || packet->payload_len < 6 + 8) return;
  if (_scan_tag == 0 || (long)(millis() - _scan_until) > 0) return;
  if (memcmp(&packet->payload[2], &_scan_tag, 4) != 0) return;

  ScanResult r = {};
  int klen = std::min<int>(packet->payload_len - 6, PUB_KEY_SIZE);
  memcpy(r.pub_key, &packet->payload[6], klen);
  if (memcmp(r.pub_key, self_id.pub_key, PUB_KEY_SIZE) == 0) return;
  for (auto& s : _scan) if (memcmp(s.pub_key, r.pub_key, 8) == 0) return;  // duplicate
  r.type = packet->payload[0] & 0x0F;
  r.snr_out = ((int8_t)packet->payload[1]) / 4.0f;
  r.snr_in = packet->getSNR();
  r.rssi = _vradio.getLastRSSI();
  _scan.push_back(r);
  {
    Heard* h = nullptr;
    for (auto& x : _heard) if (memcmp(x.pub_key, r.pub_key, PUB_KEY_SIZE) == 0) { h = &x; break; }
    if (!h) { _heard.push_back({}); h = &_heard.back(); memcpy(h->pub_key, r.pub_key, PUB_KEY_SIZE); }
    h->type = r.type;
    h->hops = 0;
    h->snr = r.snr_in;
    h->rssi = r.rssi;
    h->last_seen = getRTCClock()->getCurrentTime();
    h->count++;
    _heardChanged = true;
    _scanDirty = true;
  }
  std::string n = nameFor(r.pub_key);
  printf("%s [%s] SCAN: %-8s %02X%02X%02X%02X  %-20s  snr ud %+.1f / ind %+.1f dB, rssi %.0f\n", ts().c_str(), tag(),
         typeName(r.type), r.pub_key[0], r.pub_key[1], r.pub_key[2], r.pub_key[3], n.c_str(), r.snr_out, r.snr_in, r.rssi);
}

void VirtualNode::startScan(int secs, uint8_t filter) {
  uint8_t data[6];
  data[0] = CTL_TYPE_NODE_DISCOVER_REQ;   // prefix_only = 0 -> responders send full public key
  data[1] = filter;                        // bitmask of (1 << ADV_TYPE_xxx)
  do { getRNG()->random((uint8_t*)&_scan_tag, 4); } while (_scan_tag == 0);
  memcpy(&data[2], &_scan_tag, 4);
  _scan.clear();
  auto pkt = createControlData(data, sizeof(data));
  if (!pkt) { _scan_tag = 0; printf("   ERROR: unable to create discover request\n"); return; }
  sendZeroHop(pkt);
  _scan_until = millis() + secs * 1000UL;
  printf("[%s] scanning %d s for nearby nodes (zero hop discover)...\n", tag(), secs);
}

void VirtualNode::renameTo(const char* name) {
  if (strncmp(_prefs.node_name, name, sizeof(_prefs.node_name)) == 0) return;
  StrHelper::strncpy(_prefs.node_name, name, sizeof(_prefs.node_name));
  savePrefs();
  sendSelfAdvert(false);   // tell the neighbours the new name
}

void VirtualNode::startServiceScan() {
  if (_scan_tag) return;
  startScan(10, 0xFF);
  _serviceScan = _scan_tag != 0;
  if (_serviceScan) publishScanState(true);
}

// scan.txt: "<1 while scanning, else 0> <nodes answered so far>", for the dashboard's progress line.
void VirtualNode::publishScanState(bool active) {
  const auto temporary = path("scan.new");
  FILE* output = fopen(temporary.c_str(), "wb");
  if (!output) return;
  fprintf(output, "%d %zu\n", active ? 1 : 0, _scan.size());
  if (fclose(output) == 0) portable::replaceFile(temporary.c_str(), path("scan.txt").c_str());
  _scanDirty = false;
  _scanPublishedAt = millis();
}

void VirtualNode::publishScan() {
  const auto temporary = path("scan-results.new");
  FILE* output = fopen(temporary.c_str(), "wb");
  if (!output) return;
  size_t count = 0;
  for (const auto& result : _scan) {
    if (count++ == 100) break;
    char hex[2 * PUB_KEY_SIZE + 1];
    mesh::Utils::toHex(hex, result.pub_key, PUB_KEY_SIZE);
    fprintf(output, "%s %s | SNR %.1f / %.1f dB | RSSI %.0f dBm\n",
            typeName(result.type), hex, result.snr_out, result.snr_in, result.rssi);
  }
  if (fclose(output) == 0 && portable::replaceFile(temporary.c_str(), path("scan-results.txt").c_str())) {
    remove(path("scan").c_str());
  }
}

void VirtualNode::printScan() {
  printf("[%s] scan done: %zu node(s) answered\n", tag(), _scan.size());
  for (auto& r : _scan) {
    char hex[2 * PUB_KEY_SIZE + 1]; mesh::Utils::toHex(hex, r.pub_key, 8);
    printf("   %-8s %s  %-20s  snr ud %+5.1f  ind %+5.1f  rssi %4.0f\n", typeName(r.type), hex,
           nameFor(r.pub_key).c_str(), r.snr_out, r.snr_in, r.rssi);
  }
}

void VirtualNode::printHeard() {
  std::vector<Heard> v = _heard;
  std::sort(v.begin(), v.end(), [](const Heard& a, const Heard& b) {
    return a.hops != b.hops ? a.hops < b.hops : a.snr > b.snr; });
  uint32_t now = getRTCClock()->getCurrentTime();
  printf("[%s] %zu node(s) heard via adverts (hops 0 = direct radio range):\n", tag(), v.size());
  printf("   %-8s %-8s %-24s %4s %6s %5s %5s %s\n", "type", "id", "name", "hops", "snr", "rssi", "n", "last");
  for (auto& h : v) {
    char hex[17]; mesh::Utils::toHex(hex, h.pub_key, 4);
    printf("   %-8s %-8s %-24s %4d %+6.1f %5.0f %5u %us ago\n", typeName(h.type), hex, h.name, h.hops,
           h.snr, h.rssi, h.count, now - h.last_seen);
  }
}

// Publishes the nodes this node has heard and the names it knows, for the backbone server:
//   heard.txt: <pubkey hex> <type> <hops> <snr> <rssi> <last seen unix time> <times heard> <name hex>
//   names.txt: <pubkey hex> <name hex>      (heard nodes and contacts)
// The names are hex so that any character is safe. Written at most every 10 seconds, only after a change.
static void hexOf(char* out, const char* text) {
  static const char digits[] = "0123456789abcdef";
  size_t n = 0;
  for (const unsigned char* p = (const unsigned char*)text; *p; ++p) { out[n++] = digits[*p >> 4]; out[n++] = digits[*p & 15]; }
  out[n] = 0;
}

void VirtualNode::publishNodes() {
  const auto write = [&](const char* name, const std::string& body) {
    const std::string target = path(name), temporary = target + ".new";
    if (FILE* file = fopen(temporary.c_str(), "wb")) {
      const bool ok = fwrite(body.data(), 1, body.size(), file) == body.size();
      if (fclose(file) == 0 && ok) portable::replaceFile(temporary.c_str(), target.c_str());
    }
  };
  std::string heard, names;
  char key[2 * PUB_KEY_SIZE + 1], name[2 * 32 + 1], line[256];
  for (const auto& h : _heard) {
    mesh::Utils::toHex(key, h.pub_key, PUB_KEY_SIZE);
    hexOf(name, h.name);
    snprintf(line, sizeof(line), "%s %u %u %.1f %.0f %u %u %s\n", key, h.type, h.hops, h.snr, h.rssi, h.last_seen, h.count, name);
    heard += line;
    snprintf(line, sizeof(line), "%s %s\n", key, name);
    if (h.name[0]) names += line;
  }
  ContactInfo contact;
  for (int i = 0; i < getNumContacts(); ++i) {
    if (!getContactByIdx(i, contact) || !contact.name[0]) continue;
    mesh::Utils::toHex(key, contact.id.pub_key, PUB_KEY_SIZE);
    hexOf(name, contact.name);
    snprintf(line, sizeof(line), "%s %s\n", key, name);
    names += line;
  }
  write("heard.txt", heard);
  write("names.txt", names);
}

void VirtualNode::loop() {
  BaseChatMesh::loop();
  // While a scan runs the list is published every second, so answers show up as they arrive.
  const unsigned long every = _serviceScan ? 1000 : 10000;
  if (_service && _serviceScan && _scanDirty && millis() - _scanPublishedAt >= 1000) publishScanState(true);
  if (_service && _sentDirty && millis() - _sentPublishedAt >= 1000) publishSent();
  if (_service && _heardChanged && (_nodesPublishedAt == 0 || millis() - _nodesPublishedAt >= every)) {
    _heardChanged = false;
    _nodesPublishedAt = millis();
    publishNodes();
  }
  if ((_repeaterPending || _repeaterAdmin) && static_cast<long>(millis() - _repeaterDeadline) >= 0) {
    publishRepeater(_repeaterKey, _repeaterPending ? "Repeater login timed out." : "Administrator session expired.");
    _repeaterPending = false;
    _repeaterAdmin = false;
  }
  if (_scan_tag && (long)(millis() - _scan_until) > 0) {
    printScan();
    if (_serviceScan) { publishScan(); publishScanState(false); publishNodes(); }
    _serviceScan = false;
    _scan_tag = 0;
  }
}
