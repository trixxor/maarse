#pragma once
// A mesh::Radio implementation backed by KissHub instead of a physical LoRa chip.
#include <Dispatcher.h>
#include <deque>
#include <vector>
#include "KissHub.h"

class VirtualRadio : public mesh::Radio {
  friend class KissHub;
  KissHub& _hub;
  struct RxItem { std::vector<uint8_t> data; float snr, rssi; bool has_meta; unsigned long at; };
  std::deque<RxItem> _rxq;
  bool _sending = false, _send_done = false;
  uint32_t _seq = 0;
  float _last_snr = 0, _last_rssi = 0;

  // called by hub
  void deliver(const uint8_t* data, size_t len, unsigned long now, bool has_meta, float snr, float rssi);
  void setMetaForLast(float snr, float rssi);
  void txFinished(uint32_t seq) { if (seq == _seq) _send_done = true; }

public:
  explicit VirtualRadio(KissHub& hub) : _hub(hub) {}

  int recvRaw(uint8_t* bytes, int sz) override;
  uint32_t getEstAirtimeFor(int len_bytes) override;
  float packetScore(float snr, int packet_len) override;
  bool startSendRaw(const uint8_t* bytes, int len) override;
  bool isSendComplete() override { return _send_done; }
  void onSendFinished() override { _sending = false; _send_done = false; }
  bool isInRecvMode() const override { return !_sending; }
  float getLastRSSI() const override { return _last_rssi; }
  float getLastSNR() const override { return _last_snr; }
};
