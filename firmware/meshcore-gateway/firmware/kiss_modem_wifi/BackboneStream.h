#pragma once

#include <Arduino.h>
#include <WiFiClientSecure.h>
#include <atomic>
#include <freertos/FreeRTOS.h>
#include <freertos/task.h>
#include <memory>
#include <new>
#include <time.h>
#include "GatewayWifi.h"

class BackboneStream : public Stream {
  const GatewayBackbone* _settings = nullptr;
  std::unique_ptr<WiFiClientSecure> _client;
  WiFiClientSecure* _ready = nullptr;
  std::atomic<bool> _finished{false};
  // Pairing: the token and authority certificate the server handed out, or why not.
  char _newToken[129] = {0};
  char _newAuthority[3101] = {0};
  std::atomic<bool> _paired{false};
  std::atomic<bool> _pairFailed{false};
  std::atomic<bool> _pairWrongServer{false};
  // Why the last connection did not become a session; shown on the display.
  enum Problem : uint8_t { None, Unreachable, NoAnswer, TokenRefused, NoWorker };
  std::atomic<uint8_t> _problem{None};
  bool _connecting = false;
  uint32_t _nextAttempt = 0;

  void disconnect() {
    _client.reset();
    _nextAttempt = millis() + 5000;
  }

  static void connectTask(void* owner) {
    auto& stream = *static_cast<BackboneStream*>(owner);
    stream._ready = stream.connect().release();
    stream._finished.store(true);
    vTaskDelete(nullptr);
  }

  // Reads one line without its newline. False when it is too long or does not come in time.
  static bool readLine(WiFiClientSecure& client, char* out, size_t size, uint32_t timeoutMs) {
    size_t used = 0;
    const uint32_t started = millis();
    while (millis() - started < timeoutMs) {
      const int byte = client.read();
      if (byte < 0) {
        if (!client.connected() && !client.available()) break;
        delay(5);
      } else if (byte == '\n') {
        out[used] = 0;
        return true;
      } else if (used + 1 < size) {
        out[used++] = static_cast<char>(byte);
      } else {
        break;
      }
    }
    out[used] = 0;
    return false;
  }

  // Pairing. The token is "pair-<secret 20 hex>-<pin 32 hex>". The gateway does not
  // know the server's authority yet, so it accepts any certificate for now and
  // checks the pin (the start of the certificate's SHA-256) before it sends the
  // secret. The answer is "PAIRED <token>" and the authority certificate, or
  // "ERR". Never keeps the connection.
  void pair() {
    if (strlen(_settings->token) != 58) {   // 5 + 20 + 1 + 32: a damaged code
      _pairFailed.store(true);
      return;
    }
    char secret[21];
    memcpy(secret, _settings->token + 5, 20);
    secret[20] = 0;
    const char* pin = _settings->token + 26;
    std::unique_ptr<WiFiClientSecure> client(new (std::nothrow) WiFiClientSecure);
    if (!client) return;
    Serial.printf("[pairing] contacting %s:%u\n", _settings->host, _settings->port);
    client->setInsecure();   // the pin below is what authenticates the server
    client->setHandshakeTimeout(5);
    if (!client->connect(_settings->host, _settings->port, 3000)) {
      Serial.println("[pairing] could not connect (TCP or TLS handshake failed)");
      return;
    }
    client->setTimeout(1);
    uint8_t sha[32];
    if (!client->getFingerprintSHA256(sha)) {
      Serial.println("[pairing] no certificate from the server");
      return;
    }
    char seen[33];
    for (int i = 0; i < 16; ++i) snprintf(seen + 2 * i, 3, "%02x", sha[i]);
    if (memcmp(seen, pin, 32) != 0) {   // not the server the code was made for
      Serial.printf("[pairing] wrong server: certificate pin %.8s.. but the code says %.8s..\n", seen, pin);
      _pairWrongServer.store(true);
      _pairFailed.store(true);
      return;
    }
    char hello[48];
    const size_t length = snprintf(hello, sizeof(hello), "MAARSE-PAIR/1 %s\n", secret);
    if (client->write(reinterpret_cast<const uint8_t*>(hello), length) != length) {
      Serial.println("[pairing] could not send the secret");
      return;
    }
    Serial.println("[pairing] server verified, secret sent");
    char line[200];
    if (!readLine(*client, line, sizeof(line), 4000)) {
      Serial.printf("[pairing] no answer from the server (got \"%.20s\")\n", line);
      return;
    }
    if (strcmp(line, "ERR") == 0) {
      Serial.println("[pairing] the server refused the code (expired, used or wrong)");
      _pairFailed.store(true);   // expired, used or wrong: trying again cannot help
      return;
    }
    constexpr char paired[] = "PAIRED ";
    if (strncmp(line, paired, sizeof(paired) - 1) != 0) {
      Serial.printf("[pairing] unexpected answer \"%.20s\"\n", line);
      return;
    }
    const char* token = line + sizeof(paired) - 1;
    const size_t tokenLength = strlen(token);
    if (tokenLength < 32 || tokenLength >= sizeof(_newToken)) return;
    for (size_t i = 0; i < tokenLength; ++i) {
      const char c = token[i];
      if (!((c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '-' || c == '_')) return;
    }
    if (!readLine(*client, _newAuthority, sizeof(_newAuthority), 4000)) {
      Serial.println("[pairing] got a token but no authority certificate");
      return;
    }
    memcpy(_newToken, token, tokenLength + 1);
    Serial.println("[pairing] token and authority received");
    _paired.store(true);
  }

  std::unique_ptr<WiFiClientSecure> connect() {
    if (_settings->pairing()) {
      pair();
      return nullptr;
    }
    std::unique_ptr<WiFiClientSecure> client(new (std::nothrow) WiFiClientSecure);
    if (!client) return nullptr;
    client->setCACert(_settings->ca);
    client->setHandshakeTimeout(5);
    if (!client->connect(_settings->host, _settings->port, 3000)) {
      Serial.println("[backbone] could not connect (check the name, the port and the certificate)");
      _problem.store(Unreachable);
      return nullptr;
    }
    client->setTimeout(1);

    char hello[sizeof("MAARSE-KISS/1 \n") + sizeof(_settings->token)];
    const int length = snprintf(hello, sizeof(hello), "MAARSE-KISS/1 %s\n", _settings->token);
    if (length < 0 || static_cast<size_t>(length) >= sizeof(hello) ||
        client->write(reinterpret_cast<const uint8_t*>(hello), length) != static_cast<size_t>(length)) return nullptr;
    // "OK", or the reason: "ERR token" (the server does not know this token) or
    // "ERR worker" (authenticated, but no radio worker is connected to the server).
    char line[48];
    if (!readLine(*client, line, sizeof(line), 3000)) {
      Serial.println("[backbone] no answer from the server");
      _problem.store(NoAnswer);
      return nullptr;
    }
    if (strcmp(line, "OK") != 0) {
      Serial.printf("[backbone] the server answered \"%s\"\n", line);
      _problem.store(strcmp(line, "ERR token") == 0 ? TokenRefused : strcmp(line, "ERR worker") == 0 ? NoWorker : NoAnswer);
      return nullptr;
    }
    _problem.store(None);
    return client;
  }

public:
  // The active settings live for as long as this firmware instance. A new
  // network is committed before a TLS task may start.
  void begin(const GatewayBackbone& settings) {
    _settings = &settings;
    configTime(0, 0, "pool.ntp.org");
  }

  bool online() const { return static_cast<bool>(_client); }
  // Why the last attempt failed, in a few words for the display; null when unknown.
  const char* problem() const {
    switch (_problem.load()) {
      case Unreachable: return "Server unreachable";
      case NoAnswer: return "No answer";
      case TokenRefused: return "Token refused";
      case NoWorker: return "No radio worker";
      default: return nullptr;
    }
  }
  // Pairing: the token and the authority certificate (base64 body) the server
  // handed out, ready to be saved; or null.
  const char* pairedToken() const { return _paired.load() ? _newToken : nullptr; }
  const char* pairedAuthority() const { return _paired.load() ? _newAuthority : nullptr; }
  // True when pairing cannot work with this code: the server refused it
  // (expired, used or wrong), or it is not the server the code was made for.
  bool pairingFailed() const { return _pairFailed.load(); }
  bool pairingWrongServer() const { return _pairWrongServer.load(); }

  // A true return starts a new KISS session. The caller resets its parser
  // before consuming frames from the newly authenticated socket.
  bool poll(bool wifiOnline) {
    if (_client && (!wifiOnline || !_client->connected())) disconnect();
    if (_finished.exchange(false)) {
      _connecting = false;
      if (_ready) {
        _client.reset(_ready);
        _ready = nullptr;
        if (wifiOnline && _client->connected()) return true;
        disconnect();
      }
    }
    if (!_client && wifiOnline && _settings->configured() && !_connecting && !_paired.load() && !_pairFailed.load() &&
        static_cast<int32_t>(millis() - _nextAttempt) >= 0 && time(nullptr) >= 1700000000) {
      _nextAttempt = millis() + 5000;
      _connecting = true;
      if (xTaskCreate(connectTask, "backbone-tls", 6144, this, 1, nullptr) != pdPASS) {
        _connecting = false;
      }
    }
    return false;
  }

  int available() override { return _client ? _client->available() : 0; }
  int read() override { return _client ? _client->read() : -1; }
  int peek() override { return _client ? _client->peek() : -1; }
  int availableForWrite() override { return 1024; }
  void flush() override { if (_client) _client->flush(); }

  size_t write(uint8_t byte) override { return write(&byte, 1); }
  size_t write(const uint8_t* bytes, size_t length) override {
    if (_client && _client->write(bytes, length) != length) disconnect();
    return length;
  }
};
