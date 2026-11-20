// KISS modem over WiFi/TCP.
// Same as the official examples/kiss_modem/main.cpp, but the host link is a TCP
// socket instead of USB serial. KissModem.cpp is reused unmodified.
// Build environments are defined in src/platformio.local.ini.
#include <Arduino.h>
#include <target.h>
#include <helpers/ArduinoHelpers.h>
#include <helpers/IdentityStore.h>
#include "KissModem.h"
#include <WiFi.h>
#ifdef KISS_BACKBONE
  #include "BackboneStream.h"
#else
  #include <ESPmDNS.h>
#endif

#if !defined(KISS_BACKBONE) && !defined(WIFI_SSID)
  #error "define WIFI_SSID"
#endif
#ifndef WIFI_PWD
  #define WIFI_PWD ""
#endif
#ifndef KISS_TCP_PORT
  #define KISS_TCP_PORT 5055
#endif
#ifndef WIFI_HOSTNAME
  #define WIFI_HOSTNAME "meshcore-kiss"
#endif

// Stream proxy that always points at the currently connected TCP client.
// Only one host at a time; a new connection replaces the old one.
// While no host is connected, output is discarded so the radio keeps running.
#ifdef KISS_BACKBONE
BackboneStream tcp;
GatewayWifi wifi;
MomentaryButton setupButton(PIN_USER_BTN, 5000, true, true, false);
static bool screenPresent = false;
static bool resetArmed = false;
static bool resetError = false;
static uint32_t screenWoke = 0;
static uint32_t lastFrame = 0;

static void drawStatus() {
  if (!screenPresent || !display.isOn()) return;
  display.startFrame();
  display.setCursor(0, 0);
  display.print("Maarse gateway\n");
  if (resetArmed || resetError) {
    display.print(resetArmed ? "Release button\nto reset settings" : "Could not reset\nsettings");
  } else {
    switch (wifi.state()) {
      case GatewayWifi::State::Setup:
        display.print("Setup: maarse-gate\n192.168.4.1\nCode: ");
        display.print(wifi.code());
        break;
      case GatewayWifi::State::Connecting:
      case GatewayWifi::State::TryingNew:
      case GatewayWifi::State::Reconnecting:
        if (wifi.state() == GatewayWifi::State::TryingNew) display.print("Testing Wi-Fi\n");
        else if (wifi.state() == GatewayWifi::State::Reconnecting) display.print("Wi-Fi reconnecting\n");
        else display.print("Connecting Wi-Fi\n");
        display.drawTextEllipsized(0, 20, display.width(), wifi.ssid());
        break;
      case GatewayWifi::State::Online:
        display.drawTextEllipsized(0, 12, display.width(), wifi.ssid());
        display.setCursor(0, 24);
        display.print(WiFi.localIP().toString().c_str());
        display.setCursor(0, 38);
        if (!wifi.settings().backbone.configured()) display.print("Backbone not set");
        else display.print(tcp.online() ? "Backbone online" : "Backbone connecting");
        break;
    }
  }
  display.endFrame();
}

static void wakeScreen(uint32_t now) {
  screenWoke = now;
  if (screenPresent && !display.isOn()) display.turnOn();
  drawStatus();
  lastFrame = now;
}

static void handleButtonAndScreen() {
  const uint32_t now = millis();
  const int event = setupButton.check();
  if (event == BUTTON_EVENT_LONG_PRESS) {
    resetArmed = true;
    resetError = false;
    wakeScreen(now);
  } else if (event == BUTTON_EVENT_CLICK && !resetArmed) {
    resetError = false;
    wakeScreen(now);
  }
  // GPIO0 also selects the boot mode. Wait for the button to be released.
  if (resetArmed && !setupButton.isPressed()) {
    if (wifi.reset()) ESP.restart();
    resetArmed = false;
    resetError = true;
    wakeScreen(now);
  }
  if (!screenPresent || !display.isOn()) return;
  if (!resetArmed && now - screenWoke >= 30000) {
    display.turnOff();
  } else if (now - lastFrame >= 500) {
    lastFrame = now;
    drawStatus();
  }
}
#else
class TcpStream : public Stream {
  WiFiServer _server{KISS_TCP_PORT};
  WiFiClient _client;
public:
  void begin() { _server.begin(); _server.setNoDelay(true); }
  void poll() {
    if (_server.hasClient()) {
      WiFiClient c = _server.accept();
      if (_client && _client.connected()) _client.stop();
      _client = c;
      _client.setNoDelay(true);
      Serial.printf("[tcp] host connected from %s\n", _client.remoteIP().toString().c_str());
    }
    if (_client && !_client.connected()) { _client.stop(); Serial.println("[tcp] host disconnected"); }
  }
  bool connected() { return _client && _client.connected(); }
  int available() override { return connected() ? _client.available() : 0; }
  int read() override { return connected() ? _client.read() : -1; }
  int peek() override { return connected() ? _client.peek() : -1; }
  int availableForWrite() override { return 1024; }
  size_t write(uint8_t b) override { return write(&b, 1); }
  size_t write(const uint8_t* buf, size_t len) override {
    if (!connected()) return len;            // no host: drop
    size_t n = _client.write(buf, len);
    return n ? n : len;                      // never stall the modem on a dead socket
  }
  void flush() override { if (connected()) _client.flush(); }
};
TcpStream tcp;
#endif
#ifndef KISS_BACKBONE
static uint32_t next_wifi_check_ms = 0;

void wifiBegin() {
  WiFi.mode(WIFI_STA);
  WiFi.setHostname(WIFI_HOSTNAME);
  WiFi.setSleep(false);                      // lower latency
  WiFi.setAutoReconnect(true);
  WiFi.begin(WIFI_SSID, WIFI_PWD);
  Serial.printf("[wifi] connecting to %s", WIFI_SSID);
  uint32_t start = millis();
  while (WiFi.status() != WL_CONNECTED && millis() - start < 15000) { delay(250); Serial.print("."); }
  Serial.println();
  if (WiFi.status() == WL_CONNECTED) {
    Serial.printf("[wifi] connected, IP %s\n", WiFi.localIP().toString().c_str());
  } else {
    Serial.println("[wifi] not connected yet, will keep retrying");
  }
  MDNS.begin(WIFI_HOSTNAME);
  MDNS.addService("meshcore-kiss", "tcp", KISS_TCP_PORT);
  tcp.begin();
}

void wifiLoop() {
  tcp.poll();
  if ((int32_t)(millis() - next_wifi_check_ms) >= 0) {
    next_wifi_check_ms = millis() + 10000;
    static bool was_up = false;
    bool up = WiFi.status() == WL_CONNECTED;
    if (up && !was_up) Serial.printf("[wifi] up, IP %s\n", WiFi.localIP().toString().c_str());
    if (!up) { Serial.println("[wifi] down, reconnecting"); WiFi.reconnect(); }
    was_up = up;
  }
}

#endif

#if defined(NRF52_PLATFORM)
  #include <InternalFileSystem.h>
#elif defined(RP2040_PLATFORM)
  #include <LittleFS.h>
#elif defined(ESP32)
  #include <SPIFFS.h>
#else
  #include <InternalFileSystem.h>
#endif

#if defined(KISS_UART_RX) && defined(KISS_UART_TX)
  #include <HardwareSerial.h>
#endif

#define NOISE_FLOOR_CALIB_INTERVAL_MS 2000
#define AGC_RESET_INTERVAL_MS 30000
#define USB_TX_TIMEOUT_MS 50
#define USB_TX_BUFFER_SIZE 1024

StdRNG rng;
mesh::LocalIdentity identity;
KissModem* modem;
static uint32_t next_noise_floor_calib_ms = 0;
static uint32_t next_agc_reset_ms = 0;

void halt() {
  while (1) ;
}

void loadOrCreateIdentity() {
#if defined(NRF52_PLATFORM) || defined(STM32_PLATFORM)
  InternalFS.begin();
  IdentityStore store(InternalFS, "");
#elif defined(ESP32)
  SPIFFS.begin(true);
  IdentityStore store(SPIFFS, "/identity");
#elif defined(RP2040_PLATFORM)
  LittleFS.begin();
  IdentityStore store(LittleFS, "/identity");
  store.begin();
#else
  #error "Filesystem not defined"
#endif

  if (!store.load("_main", identity)) {
    identity = radio_new_identity();
    while (identity.pub_key[0] == 0x00 || identity.pub_key[0] == 0xFF) {
      identity = radio_new_identity();
    }
    store.save("_main", identity);
  }
}

void onSetRadio(float freq, float bw, uint8_t sf, uint8_t cr) {
  radio_driver.setParams(freq, bw, sf, cr);
}

void onSetTxPower(uint8_t power) {
  radio_driver.setTxPower(power);
}

float onGetCurrentRssi() {
  return radio_driver.getCurrentRSSI();
}

void onGetStats(uint32_t* rx, uint32_t* tx, uint32_t* errors) {
  *rx = radio_driver.getPacketsRecv();
  *tx = radio_driver.getPacketsSent();
  *errors = radio_driver.getPacketsRecvErrors();
}

void setup() {
  board.begin();

  if (!radio_init()) {
    halt();
  }

  radio_driver.begin();

  rng.begin(radio_driver.getRngSeed());
  loadOrCreateIdentity();

  sensors.begin();

  Serial.begin(115200);   // USB: log only (IP address etc.)
#ifdef KISS_BACKBONE
  setupButton.begin();
  board.periph_power.claim();
  delay(20);
  screenPresent = display.begin();
  wifi.begin(WIFI_HOSTNAME);
  tcp.begin(wifi.settings().backbone);
  wakeScreen(millis());
#else
  wifiBegin();
#endif
  modem = new KissModem(tcp, identity, rng, radio_driver, board, sensors);

  modem->setRadioCallback(onSetRadio);
  modem->setTxPowerCallback(onSetTxPower);
  modem->setGetCurrentRssiCallback(onGetCurrentRssi);
  modem->setGetStatsCallback(onGetStats);
  modem->begin();

  board.onBootComplete();
}

void loop() {
#ifdef KISS_BACKBONE
  handleButtonAndScreen();
  wifi.poll();
  if (!modem->isActuallyTransmitting() && tcp.poll(wifi.online())) modem->begin();
#else
  wifiLoop();
#endif
  modem->loop();

  if (!modem->isActuallyTransmitting() && !modem->isHostOutputBackedUp()) {
    if (!modem->isTxBusy()) {
      if ((uint32_t)(millis() - next_agc_reset_ms) >= AGC_RESET_INTERVAL_MS) {
        radio_driver.resetAGC();
        next_agc_reset_ms = millis();
      }
    }

    uint8_t rx_buf[256];
    int rx_len = radio_driver.recvRaw(rx_buf, sizeof(rx_buf));
    if (rx_len > 0) {
      int8_t snr = (int8_t)(radio_driver.getLastSNR() * 4);
      int8_t rssi = (int8_t)radio_driver.getLastRSSI();
      modem->onPacketReceived(snr, rssi, rx_buf, rx_len);
    }
  }

  board.loop();

  if ((uint32_t)(millis() - next_noise_floor_calib_ms) >= NOISE_FLOOR_CALIB_INTERVAL_MS) {
    radio_driver.triggerNoiseFloorCalibrate(0);
    next_noise_floor_calib_ms = millis();
  }
  radio_driver.loop();
}
