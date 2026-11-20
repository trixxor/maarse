#include "KissHub.h"
#include "VirtualRadio.h"
#include <Arduino.h>
#include "portable.h"
#include <cmath>
#include <cstdio>

// ---- sockets: Windows (winsock) and POSIX differ in a few small ways
#ifdef _WIN32
  #include <winsock2.h>
  #include <ws2tcpip.h>
  using NativeSocket = SOCKET;
  static const NativeSocket kNoSocket = INVALID_SOCKET;
  static uintptr_t toHandle(NativeSocket s) { return (uintptr_t)s; }
  static NativeSocket toSocket(uintptr_t h) { return (NativeSocket)h; }
  static void closeNative(NativeSocket s) { closesocket(s); }
  static bool setNonBlocking(NativeSocket s) { u_long nb = 1; return ioctlsocket(s, FIONBIO, &nb) == 0; }
  static bool wouldBlock() { return WSAGetLastError() == WSAEWOULDBLOCK; }
  static const int kSendFlags = 0;
#else
  #include <arpa/inet.h>
  #include <netdb.h>
  #include <netinet/in.h>
  #include <netinet/tcp.h>
  #include <sys/socket.h>
  using NativeSocket = int;
  static const NativeSocket kNoSocket = -1;
  // KissHub uses 0 for "no socket", but 0 is a valid descriptor: keep it one higher.
  static uintptr_t toHandle(NativeSocket s) { return (uintptr_t)s + 1; }
  static NativeSocket toSocket(uintptr_t h) { return (NativeSocket)(h - 1); }
  static void closeNative(NativeSocket s) { ::close(s); }
  static bool setNonBlocking(NativeSocket s) {
    const int flags = fcntl(s, F_GETFL, 0);
    return flags >= 0 && fcntl(s, F_SETFL, flags | O_NONBLOCK) == 0;
  }
  static bool wouldBlock() { return errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR; }
  #ifdef MSG_NOSIGNAL
    static const int kSendFlags = MSG_NOSIGNAL;   // a closed socket must not kill the process
  #else
    static const int kSendFlags = 0;
  #endif
#endif

// KISS framing (see docs/kiss_modem_protocol.md)
static const uint8_t FEND = 0xC0, FESC = 0xDB, TFEND = 0xDC, TFESC = 0xDD;
static const uint8_t CMD_DATA = 0x00, CMD_TXDELAY = 0x01, CMD_PERSIST = 0x02, CMD_SLOT = 0x03, CMD_HW = 0x06;
static const uint8_t HW_SET_RADIO = 0x09, HW_SET_TX_POWER = 0x0A, HW_GET_VERSION = 0x11;
static const uint8_t HW_RESP_VERSION = 0x91, HW_RESP_OK = 0xF0, HW_RESP_ERROR = 0xF1,
                     HW_RESP_TX_DONE = 0xF8, HW_RESP_RX_META = 0xF9;
static const uint8_t HW_ERR_TX_BUSY = 0x07;

// extra time added on top of LoRa airtime to cover USB/WiFi latency + modem CSMA
static const uint32_t TX_OVERHEAD_MS = 600;

bool KissHub::open(const std::string& port, const LoRaParams& p) {
  _p = p;
#ifdef _WIN32
  std::string path = "\.\\" + port;
  HANDLE h = CreateFileA(path.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_EXISTING, 0, nullptr);
  if (h == INVALID_HANDLE_VALUE) {
    fprintf(stderr, "Cannot open %s (error %lu)\n", port.c_str(), GetLastError());
    return false;
  }
  DCB dcb = {}; dcb.DCBlength = sizeof(dcb);
  GetCommState(h, &dcb);
  dcb.BaudRate = 115200; dcb.ByteSize = 8; dcb.Parity = NOPARITY; dcb.StopBits = ONESTOPBIT;
  dcb.fBinary = TRUE; dcb.fOutxCtsFlow = FALSE; dcb.fOutxDsrFlow = FALSE;
  dcb.fDtrControl = DTR_CONTROL_ENABLE; dcb.fRtsControl = RTS_CONTROL_ENABLE;
  dcb.fOutX = FALSE; dcb.fInX = FALSE;
  SetCommState(h, &dcb);
  COMMTIMEOUTS to = {};
  to.ReadIntervalTimeout = MAXDWORD;  // non-blocking reads
  to.WriteTotalTimeoutConstant = 500;
  SetCommTimeouts(h, &to);
  PurgeComm(h, PURGE_RXCLEAR | PURGE_TXCLEAR);
  _h = h;
  configureModem();
  return true;
#else
  fprintf(stderr, "Serial ports (%s) are only supported on Windows; use --tcp\n", port.c_str());
  return false;
#endif
}

// ---- TCP transport (firmware/kiss_modem_wifi, or the backbone server's radio worker socket)
bool KissHub::openTcp(const std::string& host_port, const LoRaParams& p) {
  _p = p;
  size_t c = host_port.rfind(':');
  _tcp_host = c == std::string::npos ? host_port : host_port.substr(0, c);
  _tcp_port = c == std::string::npos ? "5055" : host_port.substr(c + 1);
#ifdef _WIN32
  WSADATA wsa; WSAStartup(MAKEWORD(2, 2), &wsa);
#endif
  _is_tcp = true;
  if (!tcpConnect()) { fprintf(stderr, "Cannot connect to %s:%s\n", _tcp_host.c_str(), _tcp_port.c_str()); return false; }
  return true;
}

bool KissHub::tcpConnect() {
  addrinfo hints = {}, *res = nullptr;
  hints.ai_family = AF_INET; hints.ai_socktype = SOCK_STREAM;
  if (getaddrinfo(_tcp_host.c_str(), _tcp_port.c_str(), &hints, &res) != 0 || !res) return false;
  NativeSocket s = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
  bool ok = s != kNoSocket && connect(s, res->ai_addr, (int)res->ai_addrlen) == 0;
  freeaddrinfo(res);
  if (!ok) { if (s != kNoSocket) closeNative(s); return false; }
  int nd = 1; setsockopt(s, IPPROTO_TCP, TCP_NODELAY, (const char*)&nd, sizeof(nd));
  setNonBlocking(s);
  _sock = toHandle(s);
  _rx.clear(); _esc = false;
  printf("[kiss] connected to %s:%s\n", _tcp_host.c_str(), _tcp_port.c_str());
  configureModem();
  return true;
}

void KissHub::tcpLost() {
  if (_sock) { closeNative(toSocket(_sock)); _sock = 0; }
  printf("[kiss] connection to gateway lost, reconnecting...\n");
  _next_reconnect = millis() + 3000;
  if (_tx_in_flight) finishTx(false);
}

bool KissHub::isOpen() const { return _is_tcp ? _sock != 0 : _h != nullptr; }

void KissHub::rawWrite(const uint8_t* d, size_t n) {
  if (_is_tcp) {
    if (!_sock) return;
    size_t off = 0; unsigned long t0 = millis();
    while (off < n) {
      int r = (int)send(toSocket(_sock), (const char*)d + off, (int)(n - off), kSendFlags);
      if (r > 0) { off += r; continue; }
      if (wouldBlock() && millis() - t0 < 2000) { portable::sleepMs(1); continue; }
      tcpLost(); return;
    }
  } else {
#ifdef _WIN32
    DWORD w = 0;
    WriteFile((HANDLE)_h, d, (DWORD)n, &w, nullptr);
#endif
  }
}

int KissHub::rawRead(uint8_t* d, size_t n) {
  if (_is_tcp) {
    if (!_sock) return 0;
    int r = (int)recv(toSocket(_sock), (char*)d, (int)n, 0);
    if (r > 0) return r;
    if (r == 0 || !wouldBlock()) tcpLost();
    return 0;
  }
#ifdef _WIN32
  DWORD got = 0;
  if (!ReadFile((HANDLE)_h, d, (DWORD)n, &got, nullptr)) return 0;
  return (int)got;
#else
  return 0;
#endif
}

void KissHub::configureModem() {
  // radio params, power, and minimal KISS delays (the MeshCore Dispatcher already
  // does its own TX scheduling; the modem still does CSMA/LBT)
  uint8_t rp[10];
  memcpy(&rp[0], &_p.freq_hz, 4); memcpy(&rp[4], &_p.bw_hz, 4); rp[8] = _p.sf; rp[9] = _p.cr;
  writeHw(HW_SET_RADIO, rp, sizeof(rp));
  uint8_t pw = (uint8_t)_p.tx_power;
  writeHw(HW_SET_TX_POWER, &pw, 1);
  uint8_t v;
  v = 1;   writeFrame(CMD_TXDELAY, &v, 1);   // 10 ms
  v = 255; writeFrame(CMD_PERSIST, &v, 1);   // transmit as soon as channel is clear
  v = 5;   writeFrame(CMD_SLOT, &v, 1);      // 50 ms
  writeHw(HW_GET_VERSION);
}

void KissHub::close() {
#ifdef _WIN32
  if (_h) { CloseHandle((HANDLE)_h); _h = nullptr; }
#endif
  if (_sock) { closeNative(toSocket(_sock)); _sock = 0; }
}

void KissHub::writeFrame(uint8_t type, const uint8_t* data, size_t len) {
  std::vector<uint8_t> out; out.reserve(len * 2 + 3);
  out.push_back(FEND); out.push_back(type);
  for (size_t i = 0; i < len; i++) {
    uint8_t b = data[i];
    if (b == FEND) { out.push_back(FESC); out.push_back(TFEND); }
    else if (b == FESC) { out.push_back(FESC); out.push_back(TFESC); }
    else out.push_back(b);
  }
  out.push_back(FEND);
  rawWrite(out.data(), out.size());
}

void KissHub::writeHw(uint8_t sub, const uint8_t* data, size_t len) {
  std::vector<uint8_t> buf; buf.push_back(sub);
  if (len) buf.insert(buf.end(), data, data + len);
  writeFrame(CMD_HW, buf.data(), buf.size());
}

uint32_t KissHub::airtimeMillis(int len) const {
  // Semtech SX126x time-on-air formula (explicit header, CRC on)
  double tsym = std::pow(2.0, _p.sf) / (double)_p.bw_hz * 1000.0;
  int de = tsym > 16.0 ? 1 : 0;
  double tpre = (_p.preamble + 4.25) * tsym;
  double num = 8.0 * len - 4.0 * _p.sf + 28 + 16;
  double nsym = 8 + std::max(std::ceil(num / (4.0 * (_p.sf - 2 * de))) * _p.cr, 0.0);
  return (uint32_t)(tpre + nsym * tsym + 0.5);
}

void KissHub::queueTx(VirtualRadio* from, uint32_t seq, const uint8_t* bytes, int len) {
  _txq.push_back({from, seq, std::vector<uint8_t>(bytes, bytes + len)});
  startNextTx();
}

void KissHub::startNextTx() {
  if (_tx_in_flight || _txq.empty() || !isOpen()) return;
  TxItem& it = _txq.front();
  writeFrame(CMD_DATA, it.data.data(), it.data.size());
  _tx_in_flight = true;
  _tx_owner = it.from;
  _tx_deadline = millis() + airtimeMillis((int)it.data.size()) * 2 + 3000;
  if (verbose) printf("[kiss] TX %zu bytes\n", it.data.size());
}

void KissHub::finishTx(bool ok) {
  if (!_tx_in_flight) return;
  TxItem it = std::move(_txq.front());
  _txq.pop_front();
  _tx_in_flight = false;
  if (ok) tx_count++; else tx_fail++;
  if (it.from) it.from->txFinished(it.seq);
  // local loopback: the other virtual nodes "hear" this transmission, just like
  // nodes sitting next to each other on real radios would
  if (ok) {
    for (auto* r : _radios)
      if (r != it.from) r->deliver(it.data.data(), it.data.size(), millis(), true, 12.0f, -40.0f);
  }
  startNextTx();
}

void KissHub::onFrame(const uint8_t* f, size_t len) {
  if (len < 1) return;
  uint8_t type = f[0] & 0x0F;
  if (type == CMD_DATA) {
    rx_count++;
    if (verbose) printf("[kiss] RX %zu bytes\n", len - 1);
    for (auto* r : _radios) r->deliver(f + 1, len - 1, millis(), false, 0, 0);
  } else if (type == CMD_HW && len >= 2) {
    uint8_t sub = f[1];
    const uint8_t* d = f + 2; size_t dl = len - 2;
    if (sub == HW_RESP_RX_META && dl >= 2) {
      float snr = ((int8_t)d[0]) / 4.0f, rssi = (int8_t)d[1];
      for (auto* r : _radios) r->setMetaForLast(snr, rssi);
    } else if (sub == HW_RESP_TX_DONE && dl >= 1) {
      finishTx(d[0] == 0x01);
    } else if (sub == HW_RESP_ERROR && dl >= 1) {
      if (d[0] == HW_ERR_TX_BUSY && _tx_in_flight) {
        // modem was still busy: resend the same frame shortly
        _tx_in_flight = false;
        _tx_deadline = 0;
      }
      if (verbose) printf("[kiss] modem error 0x%02X\n", d[0]);
    } else if (sub == HW_RESP_VERSION && dl >= 1) {
      printf("[kiss] modem firmware v%d connected (%.3f MHz, BW %.1f kHz, SF%d, CR4/%d)\n",
             d[0], _p.freq_hz / 1e6, _p.bw_hz / 1e3, _p.sf, _p.cr);
    } else if (sub == HW_RESP_OK) {
      // ack of a SetXXX
    }
  }
}

void KissHub::loop() {
  if (_is_tcp && !_sock) {
    if ((long)(millis() - _next_reconnect) > 0) {
      if (!tcpConnect()) _next_reconnect = millis() + 3000;
    }
    return;
  }
  if (!isOpen()) return;
  uint8_t buf[512];
  int n = 0;
  while ((n = rawRead(buf, sizeof(buf))) > 0) {
    for (int i = 0; i < n; i++) {
      uint8_t b = buf[i];
      if (b == FEND) {
        if (!_rx.empty()) onFrame(_rx.data(), _rx.size());
        _rx.clear(); _esc = false;
      } else if (_esc) {
        _rx.push_back(b == TFEND ? FEND : b == TFESC ? FESC : b); _esc = false;
      } else if (b == FESC) {
        _esc = true;
      } else if (_rx.size() < 600) {
        _rx.push_back(b);
      }
    }
  }
  if (!_tx_in_flight && !_txq.empty()) startNextTx();
  if (_tx_in_flight && (long)(millis() - _tx_deadline) > 0) {
    if (verbose) printf("[kiss] TX timeout (no TxDone)\n");
    finishTx(false);
  }
}

// ------------------------------------------------------------------ VirtualRadio

void VirtualRadio::deliver(const uint8_t* data, size_t len, unsigned long now, bool has_meta, float snr, float rssi) {
  if (_rxq.size() > 32) _rxq.pop_front();
  _rxq.push_back({std::vector<uint8_t>(data, data + len), snr, rssi, has_meta, now});
}

void VirtualRadio::setMetaForLast(float snr, float rssi) {
  for (auto it = _rxq.rbegin(); it != _rxq.rend(); ++it) {
    if (!it->has_meta) { it->snr = snr; it->rssi = rssi; it->has_meta = true; return; }
  }
}

int VirtualRadio::recvRaw(uint8_t* bytes, int sz) {
  if (_rxq.empty()) return 0;
  RxItem& it = _rxq.front();
  // RxMeta follows its data frame; wait briefly for it so SNR-based logic works
  if (!it.has_meta && (millis() - it.at) < 100) return 0;
  int n = (int)std::min<size_t>(it.data.size(), (size_t)sz);
  memcpy(bytes, it.data.data(), n);
  _last_snr = it.snr; _last_rssi = it.rssi;
  _rxq.pop_front();
  return n;
}

uint32_t VirtualRadio::getEstAirtimeFor(int len_bytes) {
  return _hub.airtimeMillis(len_bytes) + TX_OVERHEAD_MS;
}

float VirtualRadio::packetScore(float snr, int packet_len) {
  // same as RadioLibWrapper::packetScoreInt()
  static const float snr_threshold[] = {-7.5f, -10, -12.5f, -15, -17.5f, -20};
  int sf = _hub.params().sf;
  if (sf < 7) return 0.0f;
  if (snr < snr_threshold[sf - 7]) return 0.0f;
  float s = (snr - snr_threshold[sf - 7]) / 10.0f;
  float c = 1 - (packet_len / 256.0f);
  return std::max(0.0f, std::min(1.0f, s * c));
}

bool VirtualRadio::startSendRaw(const uint8_t* bytes, int len) {
  _sending = true; _send_done = false;
  _hub.queueTx(this, ++_seq, bytes, len);
  return true;
}
