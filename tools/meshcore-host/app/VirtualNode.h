#pragma once
// A virtual MeshCore chat node, running the official BaseChatMesh on a PC.
// Ported from examples/simple_secure_chat/main.cpp (same commands, same contact file format).

#include <Arduino.h>
#include <Mesh.h>
#include <helpers/BaseChatMesh.h>
#include <helpers/StaticPoolPacketManager.h>
#include <helpers/SimpleMeshTables.h>
#include <helpers/AdvertDataHelpers.h>
#include <helpers/TxtDataHelpers.h>
#include <string>
#include <vector>
#include <array>
#include "VirtualRadio.h"

class SystemMillis : public mesh::MillisecondClock {
public:
  unsigned long getMillis() override { return millis(); }
};

class SystemRTC : public mesh::RTCClock {  // PC has a real clock - use it
public:
  uint32_t getCurrentTime() override;
  void setCurrentTime(uint32_t) override {}
};

class SystemRNG : public mesh::RNG {  // OS CSPRNG
public:
  void random(uint8_t* dest, size_t sz) override;
};

struct NodePrefs {
  float airtime_factor;
  char node_name[32];
  double node_lat, node_lon;
};

class VirtualNode : public BaseChatMesh, ContactVisitor {
  std::string _dir;
  NodePrefs _prefs;
  uint32_t expected_ack_crc = 0;
  ChannelDetails* _public = nullptr;
  unsigned long last_msg_sent = 0;
  ContactInfo* curr_recipient = nullptr;
  uint8_t tmp_buf[256];
  char hex_buf[512];
  VirtualRadio& _vradio;

  // ---- scan / heard
  struct Heard {
    uint8_t pub_key[PUB_KEY_SIZE];
    char name[32];
    uint8_t type, hops;
    float snr, rssi;
    uint32_t last_seen;
    uint32_t count;
  };
  std::vector<Heard> _heard;
  bool _heardChanged = false;
  bool _scanDirty = false;

  // ---- outgoing flood settings, from the person's radio.txt (path hash size, region scope)
  uint8_t _hashSize = 1;
  std::vector<std::array<uint8_t, 16>> _scopeKeys;   // one MeshCore region key per scope; empty: unscoped
  std::string _radioConfig;
  void floodOut(mesh::Packet* pkt, uint32_t delay_millis = 0);
public:
  void reloadRadio();
  void renameTo(const char* name);
private:
  // What happened to the messages sent from here: repeaters that passed a channel message on,
  // and whether a direct message was acknowledged. Published as <conversation>-heard.txt.
  struct Sent {
    std::string conversation;
    uint32_t timestamp = 0;
    uint8_t hash[MAX_HASH_SIZE] = {};
    bool tracksHash = false;
    uint32_t ack = 0;
    unsigned repeats = 0;
    float snr = -99;
    bool delivered = false;
    uint8_t seen[24] = {};
    unsigned seenCount = 0;
  };
  std::vector<Sent> _sent;
  bool _sentDirty = false;
  unsigned long _sentPublishedAt = 0;
  void trackSent(const Sent& sent);
  void publishSent();
  void logRx(mesh::Packet* packet, int len, float score) override;
  unsigned long _scanPublishedAt = 0;
  void publishScanState(bool active);
  unsigned long _nodesPublishedAt = 0;
  void publishNodes();
  struct ScanResult { uint8_t pub_key[PUB_KEY_SIZE]; uint8_t type; float snr_out, snr_in, rssi; };
  std::vector<ScanResult> _scan;
  uint32_t _scan_tag = 0;
  bool _serviceScan = false;
  bool _service = false;
  std::string _channelConfig;
  std::string _repeaterKey;
  unsigned long _repeaterDeadline = 0;
  bool _repeaterAdmin = false;
  bool _repeaterPending = false;
  bool _repeaterGuest = false;       // logged in without administrator rights: status and telemetry only
  unsigned _repeaterSeq = 0;         // counts what is published, so a client can tell a new reply from an old one
  // A binary request to the logged-in node: status, telemetry (sensor readings) or the access list.
  bool _requestPending = false;
  uint32_t _requestTag = 0;
  std::string _requestKind;
  unsigned long _requestDeadline = 0;
public:
  void repeaterData(const std::string& key, const std::string& kind);
private:
  void publishRequestResult(const std::string& kind, const std::string& body);
  void publishRepeater(const std::string& key, const char* text);
  unsigned long _scan_until = 0;
  void startScan(int secs, uint8_t filter);
  void printScan();
  void publishScan();
  void logMessage(const std::string& conversation, bool outgoing, uint32_t timestamp, const char* message);
  bool sendChannelMessage(const ChannelDetails& details, const std::string& conversation, const char* message);
  void printHeard();
  std::string nameFor(const uint8_t* pub_key) ;

  std::string path(const char* f) const { return _dir + "/" + f; }
  void loadContacts();
  void saveContacts();
  void savePrefs();
  void importCard(const char* command);
  const char* tag() const { return _prefs.node_name; }

protected:
  float getAirtimeBudgetFactor() const override { return _prefs.airtime_factor; }
  int calcRxDelay(float score, uint32_t air_time) const override { return 0; }
  bool allowPacketForward(const mesh::Packet* packet) override { return false; }  // client node: never repeat
  void onAdvertRecv(mesh::Packet* packet, const mesh::Identity& id, uint32_t timestamp, const uint8_t* app_data, size_t app_data_len) override;
  void onControlDataRecv(mesh::Packet* packet) override;

  void onDiscoveredContact(ContactInfo& contact, bool is_new, uint8_t path_len, const uint8_t* path) override;
  void onContactPathUpdated(const ContactInfo& contact) override;
  ContactInfo* processAck(const uint8_t* data) override;
  void onMessageRecv(const ContactInfo& from, mesh::Packet* pkt, uint32_t sender_timestamp, const char* text) override;
  void onCommandDataRecv(const ContactInfo&, mesh::Packet*, uint32_t, const char*) override;
  void onCLICommandRecv(const ContactInfo&, mesh::Packet*, uint32_t, const char*, char*) override {}
  void onSignedMessageRecv(const ContactInfo&, mesh::Packet*, uint32_t, const uint8_t*, const char*) override {}
  void onChannelMessageRecv(const mesh::GroupChannel& channel, mesh::Packet* pkt, uint32_t timestamp, const char* text) override;
  uint8_t onContactRequest(const ContactInfo&, uint32_t, const uint8_t*, uint8_t, uint8_t*) override { return 0; }
  void onContactResponse(const ContactInfo&, const uint8_t*, uint8_t) override;
  uint32_t calcFloodTimeoutMillisFor(uint32_t pkt_airtime_millis) const override;
  uint32_t calcDirectTimeoutMillisFor(uint32_t pkt_airtime_millis, uint8_t path_len) const override;
  void onSendTimeout() override;
  void sendFloodScoped(const ContactInfo&, mesh::Packet* pkt, uint32_t delay_millis = 0) override { floodOut(pkt, delay_millis); }
  void sendFloodScoped(const mesh::GroupChannel&, mesh::Packet* pkt, uint32_t delay_millis = 0) override { floodOut(pkt, delay_millis); }

public:
  VirtualNode(VirtualRadio& radio, mesh::RNG& rng, mesh::RTCClock& rtc)
    : BaseChatMesh(radio, *new SystemMillis(), rng, rtc, *new StaticPoolPacketManager(16), *new SimpleMeshTables()), _vradio(radio) {
    memset(&_prefs, 0, sizeof(_prefs));
    _prefs.airtime_factor = 1.0;
  }

  bool begin(const std::string& data_dir, const char* default_name, bool service = false);
  bool sendPublic(const char* message);
  void reloadChannels();
  bool sendNamedChannel(const std::string& name, const char* message);
  void repeaterRequest(bool login, const std::string& key, const char* value);
  void sendDirect(const std::string& contact, const char* message);
  const uint8_t* publicKey() const { return self_id.pub_key; }
  const char* name() const { return _prefs.node_name; }
  void showWelcome();
  void sendSelfAdvert(bool flood);
  void startServiceScan();
  void handleCommand(const char* command);
  void loop();
  void onContactVisit(const ContactInfo& contact) override;
};
