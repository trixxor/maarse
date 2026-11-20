#pragma once
// KissHub: owns the serial link to a MeshCore `kiss_modem` (official firmware) and
// multiplexes it between any number of VirtualRadio instances (one per virtual node).
//
//   LoRa  <->  Heltec (kiss_modem)  <--USB/KISS-->  KissHub  <->  VirtualRadio(s)  <->  mesh::Mesh (official core)

#include <Dispatcher.h>
#include <cstdint>
#include <deque>
#include <string>
#include <vector>

struct LoRaParams {
  uint32_t freq_hz = 869618000;  // EU/DK narrow
  uint32_t bw_hz = 62500;
  uint8_t sf = 8;
  uint8_t cr = 8;
  uint8_t preamble = 16;
  int8_t tx_power = 22;
};

class VirtualRadio;

class KissHub {
public:
  bool open(const std::string& port, const LoRaParams& p);          // USB serial (COMx)
  bool openTcp(const std::string& host_port, const LoRaParams& p);  // WiFi gateway (host:port)
  bool isOpen() const;
  void close();
  void attach(VirtualRadio* r) { _radios.push_back(r); }

  // called from VirtualRadio::startSendRaw
  void queueTx(VirtualRadio* from, uint32_t seq, const uint8_t* bytes, int len);

  // pump serial I/O; call often from the main loop
  void loop();

  uint32_t airtimeMillis(int len) const;  // pure LoRa time-on-air
  const LoRaParams& params() const { return _p; }

  bool verbose = false;
  uint32_t rx_count = 0, tx_count = 0, tx_fail = 0;

private:
  struct TxItem { VirtualRadio* from; uint32_t seq; std::vector<uint8_t> data; };

  void* _h = nullptr;  // Win32 HANDLE (serial)
  bool _is_tcp = false;
  uintptr_t _sock = 0;  // SOCKET (tcp)
  std::string _tcp_host, _tcp_port;
  unsigned long _next_reconnect = 0;
  bool tcpConnect();
  void tcpLost();
  void rawWrite(const uint8_t* d, size_t n);
  int rawRead(uint8_t* d, size_t n);
  void configureModem();
  LoRaParams _p;
  std::vector<VirtualRadio*> _radios;
  std::deque<TxItem> _txq;
  bool _tx_in_flight = false;
  VirtualRadio* _tx_owner = nullptr;
  unsigned long _tx_deadline = 0;

  // deframer
  std::vector<uint8_t> _rx;
  bool _esc = false;

  void writeFrame(uint8_t type, const uint8_t* data, size_t len);
  void writeHw(uint8_t sub, const uint8_t* data = nullptr, size_t len = 0);
  void onFrame(const uint8_t* f, size_t len);
  void finishTx(bool ok);
  void startNextTx();
};
