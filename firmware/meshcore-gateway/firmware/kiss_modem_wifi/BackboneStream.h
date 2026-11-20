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

  std::unique_ptr<WiFiClientSecure> connect() const {
    std::unique_ptr<WiFiClientSecure> client(new (std::nothrow) WiFiClientSecure);
    if (!client) return nullptr;
    client->setCACert(_settings->ca);
    client->setHandshakeTimeout(5);
    if (!client->connect(_settings->host, _settings->port, 3000)) return nullptr;
    client->setTimeout(1);

    char hello[sizeof("MAARSE-KISS/1 \n") + sizeof(_settings->token)];
    const int length = snprintf(hello, sizeof(hello), "MAARSE-KISS/1 %s\n", _settings->token);
    if (length < 0 || static_cast<size_t>(length) >= sizeof(hello) ||
        client->write(reinterpret_cast<const uint8_t*>(hello), length) != static_cast<size_t>(length)) return nullptr;
    constexpr char accepted[] = "OK\n";
    size_t received = 0;
    const uint32_t started = millis();
    while (received < sizeof(accepted) - 1 && millis() - started < 2000) {
      const int byte = client->read();
      if (byte < 0) {
        if (!client->connected()) return nullptr;
        delay(5);
      } else if (byte != accepted[received++]) {
        return nullptr;
      }
    }
    if (received != sizeof(accepted) - 1) return nullptr;
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
    if (!_client && wifiOnline && _settings->configured() && !_connecting &&
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
