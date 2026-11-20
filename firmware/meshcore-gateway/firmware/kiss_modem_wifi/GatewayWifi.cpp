#ifdef KISS_BACKBONE
#include "GatewayWifi.h"

#include <esp_system.h>

#if __has_include("backbone_config.h")
  #include "backbone_config.h"
  #define HAS_FACTORY_BACKBONE 1
#endif

namespace {
constexpr char SetupSsid[] = "maarse-gate";
constexpr char ActiveKey[] = "active";
constexpr char TrialKey[] = "trial";
constexpr uint32_t SettingsVersion = 1;
constexpr uint32_t WifiTimeoutMs = 30000;
const IPAddress SetupIp(192, 168, 4, 1);

const char SetupPage[] PROGMEM = R"HTML(<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Maarse gateway setup</title>
<style>body{font:16px system-ui;max-width:34em;margin:2em auto;padding:0 1em;color:#17241e}label{display:block;margin:.8em 0}input,select,textarea{font:inherit;box-sizing:border-box;width:100%;padding:.5em}button{font:inherit;padding:.5em .8em;margin:.4em 0}summary{cursor:pointer}small{display:block;margin:.4em 0}</style></head><body>
<h1>Maarse gateway</h1><p id="notice" role="status"></p>
<form method="post" action="/save">
<label>Nearby Wi-Fi networks <select id="networks"><option value="">Scanning...</option></select></label>
<button id="scan" type="button">Scan again</button>
<label>Network name (SSID) <input id="ssid" name="ssid" required maxlength="32" autocomplete="off"></label>
<label>Wi-Fi password <input name="password" type="password" maxlength="64" autocomplete="off"></label>
<small>Leave the password empty for an open network. Use a 2.4 GHz network.</small>
<details><summary>Backbone connection</summary>
<small>Paste the pairing code from the backbone dashboard (Connections, Pair a gateway). It works once, for ten minutes. The gateway then fetches its own token and restarts. Leave everything blank to keep the saved backbone settings.</small>
<label>Pairing code <textarea name="pairing" rows="2" maxlength="200" autocomplete="off" spellcheck="false"></textarea></label>
<small>Or fill in all of the following fields by hand instead of a pairing code.</small>
<label>Server DNS name <input name="host" maxlength="253" autocomplete="off"></label>
<label>Port <input name="port" type="number" min="1" max="65535" placeholder="5056"></label>
<label>Gateway token <input name="token" type="password" maxlength="128" autocomplete="off"></label>
<label>Issuing CA certificate (PEM) <textarea name="ca" rows="5" maxlength="4096" autocomplete="off"></textarea></label>
</details>
<label>Setup code from the gateway display <input name="code" required inputmode="numeric" pattern="[0-9]{6}" maxlength="6" autocomplete="off"></label>
<button>Save and restart</button></form>
<script>
const list = document.getElementById('networks');
const ssid = document.getElementById('ssid');
let timer;
async function networks() {
  clearTimeout(timer);
  try {
    const response = await fetch('/networks', { cache: 'no-store' });
    if (response.status === 202) { timer = setTimeout(networks, 750); return; }
    if (!response.ok) throw Error();
    list.innerHTML = await response.text();
  } catch { list.textContent = ''; list.add(new Option('Scan unavailable. Enter the SSID manually.', '')); }
}
list.onchange = () => { if (list.value) ssid.value = list.value; };
document.getElementById('scan').onclick = async () => {
  list.textContent = ''; list.add(new Option('Scanning...', ''));
  try { await fetch('/scan', { method: 'POST' }); } finally { networks(); }
};
fetch('/notice').then(response => response.text()).then(text => { document.getElementById('notice').textContent = text; });
networks();
</script></body></html>)HTML";

template <size_t Size>
bool copyField(char (&out)[Size], const String& value) {
  if (value.length() >= Size || strnlen(value.c_str(), Size) != value.length()) return false;
  memcpy(out, value.c_str(), value.length() + 1);
  return true;
}

template <size_t Size>
bool terminated(const char (&value)[Size]) {
  return memchr(value, '\0', Size) != nullptr;
}

bool asciiAlnum(char value) {
  return (value >= '0' && value <= '9') || (value >= 'A' && value <= 'Z') || (value >= 'a' && value <= 'z');
}

bool validDns(const String& host) {
  if (host.isEmpty() || host.length() > 253) return false;
  size_t labelLength = 0;
  char previous = '.';
  for (size_t i = 0; i < host.length(); ++i) {
    const char value = host[i];
    if (value == '.') {
      if (!labelLength || previous == '-') return false;
      labelLength = 0;
    } else if ((!asciiAlnum(value) && value != '-') || (!labelLength && value == '-') || ++labelLength > 63) {
      return false;
    }
    previous = value;
  }
  return labelLength && previous != '-';
}

bool validToken(const String& token) {
  if (token.length() < 32 || token.length() > 128) return false;
  for (size_t i = 0; i < token.length(); ++i) {
    const char value = token[i];
    if (!asciiAlnum(value) && value != '-' && value != '_') return false;
  }
  return true;
}

bool lowerHex(const String& value, size_t length) {
  if (value.length() != length) return false;
  for (size_t i = 0; i < length; ++i) {
    const char c = value[i];
    if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) return false;
  }
  return true;
}

bool base64Body(const String& body) {
  if (body.length() < 100 || body.length() > 3000) return false;
  for (size_t i = 0; i < body.length(); ++i) {
    const char c = body[i];
    if (!asciiAlnum(c) && c != '+' && c != '/' && c != '=') return false;
  }
  return true;
}

// A pairing code from the backbone dashboard: MP1;host;port;secret;pin.
// secret: 20 hex characters. pin: 32 hex characters, the start of the SHA-256 of
// the server's certificate. The gateway keeps "pair-<secret>-<pin>" as its token
// until the server has given it a token and its authority certificate.
bool parsePairing(const String& code, GatewayBackbone& out) {
  String parts[5];
  size_t count = 0, start = 0;
  for (size_t i = 0; i <= code.length(); ++i) {
    if (i == code.length() || code[i] == ';') {
      if (count == 5) return false;
      parts[count++] = code.substring(start, i);
      start = i + 1;
    }
  }
  if (count != 5 || parts[0] != "MP1") return false;
  String host = parts[1];
  host.toLowerCase();
  const String& secret = parts[3];
  const String& pin = parts[4];
  char* end = nullptr;
  const unsigned long port = strtoul(parts[2].c_str(), &end, 10);
  if (parts[2].isEmpty() || *end || !port || port > 65535) return false;
  if (!validDns(host) || !lowerHex(secret, 20) || !lowerHex(pin, 32)) return false;
  out.port = static_cast<uint16_t>(port);
  return copyField(out.host, host) && copyField(out.token, String("pair-") + secret + "-" + pin);
}

void appendHtml(String& out, const String& input) {
  for (size_t i = 0; i < input.length(); ++i) {
    switch (input[i]) {
      case '&': out += F("&amp;"); break;
      case '<': out += F("&lt;"); break;
      case '>': out += F("&gt;"); break;
      case '"': out += F("&quot;"); break;
      case '\'': out += F("&#39;"); break;
      default: if (static_cast<unsigned char>(input[i]) >= 0x20) out += input[i];
    }
  }
}
}

void GatewayWifi::begin(const char* hostname) {
  _hostname = hostname;
  _storeReady = _prefs.begin("maarse-gateway", false);
  WiFi.persistent(false);
  WiFi.setAutoReconnect(false);
  _http.on("/", HTTP_GET, [this] {
    if (!fromPortal()) return;
    _http.sendHeader("Cache-Control", "no-store");
    _http.send_P(200, "text/html; charset=utf-8", SetupPage);
  });
  _http.on("/notice", HTTP_GET, [this] {
    if (fromPortal()) _http.send(200, "text/plain", _notice);
  });
  _http.on("/networks", HTTP_GET, [this] { if (fromPortal()) networks(); });
  _http.on("/scan", HTTP_POST, [this] {
    if (!fromPortal()) return;
    scan();
    _http.send(202, "text/plain", "Scanning");
  });
  _http.on("/save", HTTP_POST, [this] { if (fromPortal()) save(); });
  _http.onNotFound([this] {
    if (!fromPortal()) return;
    _http.sendHeader("Location", "http://192.168.4.1/");
    _http.send(302, "text/plain", "Gateway setup");
  });
  const char* headers[] = {"Origin"};
  _http.collectHeaders(headers, 1);

  const bool known = load(ActiveKey, _active);
  if (load(TrialKey, _trial)) connect(State::TryingNew);
  else if (known) connect(State::Connecting);
  else startPortal(_storeReady ? "Select a Wi-Fi network to get started." : "Could not open settings storage.");
}

bool GatewayWifi::load(const char* key, GatewaySettings& out) {
  if (!_storeReady || !_prefs.isKey(key) || _prefs.getBytesLength(key) != sizeof(out)) return false;
  const bool valid = _prefs.getBytes(key, &out, sizeof(out)) == sizeof(out) &&
      out.version == SettingsVersion && out.ssid[0] && terminated(out.ssid) && terminated(out.password) &&
      terminated(out.backbone.host) && terminated(out.backbone.token) && terminated(out.backbone.ca);
  if (!valid) memset(&out, 0, sizeof(out));
  return valid;
}

void GatewayWifi::connect(State next) {
  const auto& settings = next == State::TryingNew ? _trial : _active;
  WiFi.mode(WIFI_STA);
  WiFi.setHostname(_hostname);
  WiFi.setSleep(false);
  WiFi.disconnect(false, false);
  WiFi.begin(settings.ssid, settings.password[0] ? settings.password : nullptr);
  _state = next;
  _started = millis();
}

void GatewayWifi::startPortal(const char* notice) {
  _state = State::Setup;
  _notice = notice;
  _started = millis();
  WiFi.disconnect(false, false);
  WiFi.mode(WIFI_AP_STA);
  WiFi.softAPConfig(SetupIp, SetupIp, IPAddress(255, 255, 255, 0));
  if (!WiFi.softAP(SetupSsid, nullptr, 1, false, 4)) return;
  newCode();
  _dns.start(53, "*", SetupIp);
  _http.begin();
  _portalRunning = true;
  Serial.printf("[setup] Join %s, open http://192.168.4.1/, setup code %s\n", SetupSsid, _code);
  scan();
}

void GatewayWifi::poll() {
  const uint32_t now = millis();
  if (_state == State::Setup) {
    if (_saved && now - _savedAt >= 600) ESP.restart();
    if (_portalRunning) {
      _dns.processNextRequest();
      _http.handleClient();
    } else if (now - _started >= 5000) {
      startPortal(_notice);
    }
    return;
  }

  const bool connected = WiFi.status() == WL_CONNECTED && WiFi.localIP() != IPAddress(0, 0, 0, 0);
  if (_state == State::Online) {
    if (!connected) connect(State::Reconnecting);
    return;
  }
  if (connected) {
    if (_state == State::TryingNew) {
      if (_prefs.putBytes(ActiveKey, &_trial, sizeof(_trial)) != sizeof(_trial)) {
        _prefs.remove(TrialKey);
        if (_active.ssid[0]) connect(State::Connecting);
        else startPortal("Could not save the new settings. Try again.");
        return;
      }
      _active = _trial;
      _prefs.remove(TrialKey);
      memset(&_trial, 0, sizeof(_trial));
    }
    _state = State::Online;
    Serial.printf("[wifi] Connected, IP %s\n", WiFi.localIP().toString().c_str());
    return;
  }
  if (now - _started < WifiTimeoutMs) return;
  if (_state == State::TryingNew) {
    _prefs.remove(TrialKey);
    memset(&_trial, 0, sizeof(_trial));
    if (_active.ssid[0]) {
      _notice = "New network failed. Trying previous settings.";
      connect(State::Connecting);
      return;
    }
  }
  startPortal("Could not connect to Wi-Fi. Choose a network and try again.");
}

bool GatewayWifi::reset() {
  return _storeReady && _prefs.clear();
}

void GatewayWifi::newCode() {
  snprintf(_code, sizeof(_code), "%06lu", static_cast<unsigned long>(esp_random() % 1000000));
}

bool GatewayWifi::fromPortal() {
  if (_state == State::Setup && _http.client().localIP() == SetupIp) return true;
  _http.send(403, "text/plain", "Setup is only available on the gateway Wi-Fi.");
  return false;
}

void GatewayWifi::scan() {
  if (WiFi.scanComplete() == WIFI_SCAN_RUNNING) return;
  WiFi.scanDelete();
  WiFi.scanNetworks(true, false);
}

void GatewayWifi::networks() {
  const int count = WiFi.scanComplete();
  _http.sendHeader("Cache-Control", "no-store");
  if (count == WIFI_SCAN_RUNNING) {
    _http.send(202, "text/plain", "Scanning");
    return;
  }
  String options = F("<option value=\"\">Choose a network or enter its name</option>");
  for (int i = 0; i < count && i < 32; ++i) {
    const String name = WiFi.SSID(i);
    if (name.isEmpty()) continue;
    bool seen = false;
    for (int j = 0; j < i; ++j) if (WiFi.SSID(j) == name) { seen = true; break; }
    if (seen) continue;
    options += F("<option value=\"");
    appendHtml(options, name);
    options += F("\">");
    appendHtml(options, name);
    options += F(" ("); options += WiFi.RSSI(i); options += F(" dBm)");
    if (WiFi.encryptionType(i) != WIFI_AUTH_OPEN) options += F(" - secured");
    options += F("</option>");
  }
  _http.send(200, "text/html; charset=utf-8", options);
}

bool GatewayWifi::completePairing(const char* token, const char* authorityBody) {
  const String body(authorityBody);
  if (!_storeReady || !validToken(String(token)) || !base64Body(body)) return false;
  String pem = F("-----BEGIN CERTIFICATE-----\n");
  for (size_t i = 0; i < body.length(); i += 64) {
    pem += body.substring(i, i + 64 < body.length() ? i + 64 : body.length());
    pem += '\n';
  }
  pem += F("-----END CERTIFICATE-----\n");
  // Changed in place: a copy of the settings is 4.5 KB, too much for this stack.
  if (!copyField(_active.backbone.token, String(token)) || !copyField(_active.backbone.ca, pem)) return false;
  return _prefs.putBytes(ActiveKey, &_active, sizeof(_active)) == sizeof(_active);
}

bool GatewayWifi::backboneForm(GatewayBackbone& target) {
  String pairing = _http.arg("pairing");
  pairing.trim();
  if (!pairing.isEmpty()) return parsePairing(pairing, target);
  String host = _http.arg("host"), port = _http.arg("port"), token = _http.arg("token"), ca = _http.arg("ca");
  host.trim(); port.trim(); token.trim(); ca.trim();
  if (host.isEmpty() && port.isEmpty() && token.isEmpty() && ca.isEmpty()) {
    target = _active.backbone;
#ifdef HAS_FACTORY_BACKBONE
    if (!target.configured()) {
      if (!copyField(target.host, String(BACKBONE_HOST)) || !copyField(target.token, String(BACKBONE_GATEWAY_TOKEN)) ||
          !copyField(target.ca, String(BACKBONE_CA_PEM))) return false;
      target.port = BACKBONE_PORT;
    }
#endif
    return true;
  }
  unsigned long number = 5056;
  if (!port.isEmpty()) {
    char* end = nullptr;
    number = strtoul(port.c_str(), &end, 10);
    if (*end || port[0] < '0' || port[0] > '9' || !number || number > 65535) return false;
  }
  if (!validDns(host) || !validToken(token) || !ca.startsWith("-----BEGIN CERTIFICATE-----") ||
      !ca.endsWith("-----END CERTIFICATE-----")) return false;
  ca += '\n';
  target.port = static_cast<uint16_t>(number);
  return copyField(target.host, host) && copyField(target.token, token) && copyField(target.ca, ca);
}

void GatewayWifi::save() {
  if (_saved) {
    _http.send(409, "text/plain", "Settings already saved. The gateway is restarting.");
    return;
  }
  const String origin = _http.header("Origin");
  const uint32_t now = millis();
  if (!origin.isEmpty() && origin != "http://192.168.4.1") {
    _http.send(403, "text/plain", "Open http://192.168.4.1/ to configure the gateway.");
    return;
  }
  if (_posted && now - _lastPost < 350) {
    _http.send(429, "text/plain", "Wait a moment and try again.");
    return;
  }
  _posted = true;
  _lastPost = now;
  if (_http.arg("code") != _code) {
    // Six digits can be guessed unless the code changes. After five wrong
    // codes a new one is generated and shown on the display.
    if (++_codeFailures >= 5) {
      newCode();
      _codeFailures = 0;
      Serial.println("[setup] Too many wrong codes; a new setup code is shown on the display");
      _http.send(403, "text/plain", "Too many wrong codes. Read the new setup code from the gateway display.");
      return;
    }
    _http.send(403, "text/plain", "Incorrect setup code. Press the button to wake the display.");
    return;
  }
  _codeFailures = 0;
  memset(&_trial, 0, sizeof(_trial));
  _trial.version = SettingsVersion;
  const String ssid = _http.arg("ssid");
  const String password = _http.arg("password");
  if (ssid.isEmpty() || !copyField(_trial.ssid, ssid) || !copyField(_trial.password, password) ||
      (!password.isEmpty() && password.length() < 8) || !backboneForm(_trial.backbone)) {
    _http.send(400, "text/plain", "Check the network name, Wi-Fi password and any backbone settings, then try again.");
    return;
  }
  if (!_storeReady || _prefs.putBytes(TrialKey, &_trial, sizeof(_trial)) != sizeof(_trial)) {
    _http.send(500, "text/plain", "Could not save settings. The existing Wi-Fi settings are unchanged.");
    return;
  }
  _http.send(200, "text/plain", "Settings saved. The gateway is restarting to test the Wi-Fi. If it cannot connect, it will try the previous network or reopen maarse-gate.");
  _savedAt = millis();
  _saved = true;
}

#endif
