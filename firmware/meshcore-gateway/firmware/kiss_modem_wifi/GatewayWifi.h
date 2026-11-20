#pragma once

#include <Arduino.h>
#include <DNSServer.h>
#include <Preferences.h>
#include <WebServer.h>
#include <WiFi.h>

struct GatewayBackbone {
  char host[254];
  uint16_t port;
  char token[129];
  char ca[4097];

  bool configured() const { return host[0] && port && token[0] && ca[0]; }
};

struct GatewaySettings {
  uint32_t version;
  char ssid[33];
  char password[65];
  GatewayBackbone backbone;
};

class GatewayWifi {
public:
  enum class State { Setup, Connecting, TryingNew, Reconnecting, Online };

  void begin(const char* hostname);
  void poll();
  bool reset();
  State state() const { return _state; }
  bool online() const { return _state == State::Online && WiFi.status() == WL_CONNECTED; }
  const GatewaySettings& settings() const { return _active; }
  const char* ssid() const { return _state == State::TryingNew ? _trial.ssid : _active.ssid; }
  const char* code() const { return _code; }
  const char* notice() const { return _notice; }

private:
  Preferences _prefs;
  WebServer _http{80};
  DNSServer _dns;
  GatewaySettings _active{};
  GatewaySettings _trial{};
  State _state = State::Setup;
  const char* _hostname = nullptr;
  const char* _notice = "";
  char _code[7]{};
  uint32_t _started = 0;
  uint32_t _savedAt = 0;
  uint32_t _lastPost = 0;
  uint8_t _codeFailures = 0;
  bool _saved = false;
  bool _posted = false;
  bool _storeReady = false;
  bool _portalRunning = false;

  bool load(const char* key, GatewaySettings& out);
  void connect(State next);
  void startPortal(const char* notice);
  void scan();
  void networks();
  void save();
  void newCode();
  bool fromPortal();
  bool backboneForm(GatewayBackbone& target);
};
